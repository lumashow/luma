//! How much of a subscription's allowance is used: the "Session 42% · Week
//! 17%" a CLI's own usage screen shows.
//!
//! Each service is asked the way its CLI asks. Codex's app server answers it
//! directly. Claude Code has no command for it, so its usage endpoint is read
//! with the access token the CLI saved — only read, and sent only to
//! Anthropic. The token is never refreshed here: a refresh token is
//! single-use, and spending the CLI's would sign the CLI out. Antigravity
//! answers through its `/usage` command, with a pair of windows for each
//! group of models that share an allowance.

use super::{antigravity, catalog::Service, claude, codex, protocol, AgentError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// A subscription's rolling windows, shortest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanUsage {
    pub windows: Vec<Window>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// `Session`, `Week` or `Month`.
    pub label: String,
    /// The models that share this window, where a plan meters groups of
    /// models apart: `Gemini`, `Claude and GPT`. `None` when the plan has one
    /// allowance for everything.
    #[serde(default)]
    pub group: Option<String>,
    /// The part used, 0 to 1.
    pub used: f32,
    /// When the window starts over, in Unix seconds.
    pub resets_at: Option<i64>,
}

/// A reading is good for this long: asking Codex starts its CLI, and asking
/// Claude reads the Keychain.
const FRESH: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(15);

fn cache() -> &'static Mutex<HashMap<Service, (Instant, PlanUsage)>> {
    static CACHE: OnceLock<Mutex<HashMap<Service, (Instant, PlanUsage)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// Whether `service` has plan usage to report, so a host can leave room for
/// it before it is read.
pub fn reported_by(service: Service) -> bool {
    service.provider().is_none()
}

/// `service`'s plan usage, or `None` for a service that has none to report:
/// an API gateway, which bills by the token.
///
/// # Errors
///
/// When the service cannot be asked: its CLI is missing or signed out, or the
/// answer is not one this build reads.
pub async fn read(
    service: Service,
    cwd: &std::path::Path,
) -> Result<Option<PlanUsage>, AgentError> {
    if !reported_by(service) {
        return Ok(None);
    }
    if let Some((at, usage)) = cache().lock().expect("plan usage").get(&service) {
        if at.elapsed() < FRESH {
            return Ok(Some(usage.clone()));
        }
    }
    let asked = async {
        match service {
            Service::Claude => claude::plan_usage().await.map(|body| claude_windows(&body)),
            Service::Codex => codex::rate_limits(cwd)
                .await
                .map(|limits| codex_windows(&limits)),
            Service::Antigravity => antigravity::usage(cwd)
                .await
                .map(|usage| antigravity_windows(&usage)),
            Service::OpenRouter | Service::Vercel => {
                unreachable!("only the CLI subscriptions are asked")
            }
        }
    };
    let usage = tokio::time::timeout(TIMEOUT, asked)
        .await
        .map_err(|_| protocol(format!("{} usage timed out", service.label())))??
        .ok_or_else(|| protocol(format!("{} reported no usage windows", service.label())))?;
    cache()
        .lock()
        .expect("plan usage")
        .insert(service, (Instant::now(), usage.clone()));
    Ok(Some(usage))
}

/// Anthropic's usage answer: `five_hour` and `seven_day`, each a
/// `utilization` in percent and an RFC 3339 `resets_at`, or null when the
/// plan has no such window.
fn claude_windows(body: &Value) -> Option<PlanUsage> {
    let windows: Vec<_> = [("five_hour", "Session"), ("seven_day", "Week")]
        .into_iter()
        .filter_map(|(key, label)| {
            let window = &body[key];
            Some(Window {
                label: label.into(),
                group: None,
                used: fraction(window["utilization"].as_f64()?),
                resets_at: window["resets_at"]
                    .as_str()
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.timestamp()),
            })
        })
        .collect();
    (!windows.is_empty()).then_some(PlanUsage { windows })
}

/// Codex's `account/rateLimits/read` answer: `rateLimits.primary` and
/// `.secondary`, each a `usedPercent`, a `windowDurationMins` and a
/// `resetsAt` in Unix seconds. A window is named by its length.
fn codex_windows(result: &Value) -> Option<PlanUsage> {
    const DAY: u64 = 24 * 60;
    let mut windows: Vec<_> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| {
            let window = &result["rateLimits"][key];
            let minutes = window["windowDurationMins"].as_u64().unwrap_or(0);
            let label = match minutes {
                m if m >= 28 * DAY => "Month",
                m if m >= 5 * DAY => "Week",
                _ => "Session",
            };
            Some((
                minutes,
                Window {
                    label: label.into(),
                    group: None,
                    used: fraction(window["usedPercent"].as_f64()?),
                    resets_at: window["resetsAt"].as_i64(),
                },
            ))
        })
        .collect();
    windows.sort_by_key(|(minutes, _)| *minutes);
    let windows: Vec<_> = windows.into_iter().map(|(_, window)| window).collect();
    (!windows.is_empty()).then_some(PlanUsage { windows })
}

/// Antigravity's `/usage` answer: `groups`, each a `name` and its `buckets`,
/// each a `window` (`5h` or `weekly`), the `remaining_fraction` and an RFC
/// 3339 `reset_time`. A group is named without the word every group ends in.
fn antigravity_windows(usage: &Value) -> Option<PlanUsage> {
    let mut windows = Vec::new();
    for group in usage["groups"].as_array().into_iter().flatten() {
        let name = group["name"].as_str().map(|name| {
            let words: Vec<_> = name.split_whitespace().collect();
            match words.split_last() {
                Some((last, rest)) if last.eq_ignore_ascii_case("models") => rest.join(" "),
                _ => name.to_string(),
            }
        });
        let mut buckets: Vec<_> = group["buckets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|bucket| {
                let (order, label) = match bucket["window"].as_str()? {
                    "5h" => (0, "Session"),
                    "weekly" => (1, "Week"),
                    _ => return None,
                };
                Some((
                    order,
                    Window {
                        label: label.into(),
                        group: name.clone(),
                        used: fraction(100.0 * (1.0 - bucket["remaining_fraction"].as_f64()?)),
                        resets_at: bucket["reset_time"]
                            .as_str()
                            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                            .map(|at| at.timestamp()),
                    },
                ))
            })
            .collect();
        buckets.sort_by_key(|(order, _)| *order);
        windows.extend(buckets.into_iter().map(|(_, window)| window));
    }
    (!windows.is_empty()).then_some(PlanUsage { windows })
}

impl PlanUsage {
    /// The windows `model` spends from: its group's where the plan meters
    /// groups apart (the group whose name has a word of the model's id in
    /// it), and all of them otherwise.
    pub fn for_model(&self, model: Option<&str>) -> Vec<&Window> {
        let model = model.unwrap_or_default().to_lowercase();
        let spends = |window: &&Window| {
            window.group.as_deref().is_none_or(|group| {
                group.split_whitespace().any(|word| {
                    word.len() > 2 && word != "and" && model.contains(&word.to_lowercase())
                })
            })
        };
        let own: Vec<_> = self.windows.iter().filter(spends).collect();
        if own.is_empty() {
            self.windows.iter().collect()
        } else {
            own
        }
    }
}

fn fraction(percent: f64) -> f32 {
    (percent / 100.0).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Claude's answer as the API gave it on 2026-11-17: a plan with no
    /// five-hour window reports only its week.
    #[test]
    fn claude_windows_are_the_ones_the_plan_has() {
        let usage = claude_windows(&json!({
            "five_hour": null,
            "seven_day": {"utilization": 47.0, "resets_at": "2026-11-18T09:00:00.290088+00:00"},
            "seven_day_opus": null,
        }))
        .expect("a week");
        assert_eq!(usage.windows.len(), 1);
        let week = &usage.windows[0];
        assert_eq!(week.label, "Week");
        assert!((week.used - 0.47).abs() < 1e-6);
        let both = claude_windows(&json!({
            "five_hour": {"utilization": 120.0, "resets_at": "2026-11-17T23:00:00Z"},
            "seven_day": {"utilization": 3.5, "resets_at": "2026-11-18T09:00:00Z"},
        }))
        .expect("both");
        let labels: Vec<_> = both.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Session", "Week"]);
        // Over the allowance is still a full meter, and the session resets
        // before the week does.
        assert!(both.windows[0].used <= 1.0);
        assert!(both.windows[0].resets_at < both.windows[1].resets_at);
        assert_eq!(claude_windows(&json!({"five_hour": null})), None);
    }

    /// Codex names no window; its length does. The shorter comes first
    /// whichever slot it was reported in.
    #[test]
    fn codex_windows_are_named_by_their_length() {
        let usage = codex_windows(&json!({"rateLimits": {
            "primary": {"usedPercent": 12, "windowDurationMins": 10080, "resetsAt": 1795584609},
            "secondary": {"usedPercent": 80, "windowDurationMins": 300, "resetsAt": 1795000000},
        }}))
        .expect("two windows");
        let read: Vec<_> = usage
            .windows
            .iter()
            .map(|w| (w.label.as_str(), (w.used * 100.0).round() as u32))
            .collect();
        assert_eq!(read, [("Session", 80), ("Week", 12)]);
        assert_eq!(usage.windows[1].resets_at, Some(1795584609));
        assert_eq!(
            codex_windows(&json!({"rateLimits": {"primary": null, "secondary": null}})),
            None
        );
    }

    /// Antigravity's answer as `agy` 1.5.10 gave it: two groups of models,
    /// each with its session and its week, reported as what remains. A model
    /// spends from its own group's windows.
    #[test]
    fn antigravity_windows_are_grouped_by_the_models_that_share_them() {
        let usage = antigravity_windows(&json!({"groups": [
            {"name": "Gemini Models", "buckets": [
                {"window": "weekly", "remaining_fraction": 0.75, "reset_time": "2026-11-25T05:13:44Z"},
                {"window": "5h", "remaining_fraction": 0.5, "reset_time": "2026-11-18T10:13:44Z"},
            ]},
            {"name": "Claude and GPT models", "buckets": [
                {"window": "weekly", "remaining_fraction": 1, "reset_time": "2026-11-25T05:55:01Z"},
                {"window": "5h", "remaining_fraction": 1, "reset_time": "2026-11-18T10:55:01Z"},
            ]},
        ]}))
        .expect("four windows");
        let read = |windows: Vec<&Window>| -> Vec<_> {
            windows
                .iter()
                .map(|w| {
                    let group = w.group.clone().unwrap_or_default();
                    (group, w.label.clone(), (w.used * 100.0).round() as u32)
                })
                .collect()
        };
        let gemini = [
            ("Gemini".to_string(), "Session".to_string(), 50),
            ("Gemini".to_string(), "Week".to_string(), 25),
        ];
        let other = [
            ("Claude and GPT".to_string(), "Session".to_string(), 0),
            ("Claude and GPT".to_string(), "Week".to_string(), 0),
        ];
        assert_eq!(read(usage.for_model(Some("gemini-4-carbon"))), gemini);
        assert_eq!(
            read(usage.for_model(Some("gemini-pro-orchestrator"))),
            gemini
        );
        assert_eq!(read(usage.for_model(Some("claude-sonnet-4-6"))), other);
        assert_eq!(read(usage.for_model(Some("gpt-oss-120b"))), other);
        // A model no group names is shown every window rather than none.
        assert_eq!(usage.for_model(None).len(), 4);
        assert_eq!(antigravity_windows(&json!({"groups": []})), None);
    }

    #[tokio::test]
    #[ignore = "reads the usage of the subscriptions signed in on this machine"]
    async fn live_subscriptions_report_their_usage() {
        let cwd = tempfile::tempdir().unwrap();
        for service in [Service::Claude, Service::Codex, Service::Antigravity] {
            let usage = read(service, cwd.path()).await.unwrap().expect("a plan");
            assert!(!usage.windows.is_empty());
            for window in &usage.windows {
                assert!((0.0..=1.0).contains(&window.used));
                eprintln!("{}: {window:?}", service.label());
            }
        }
    }

    #[tokio::test]
    async fn a_service_billed_by_the_token_has_no_plan_usage() {
        let cwd = tempfile::tempdir().unwrap();
        for service in Service::ALL
            .into_iter()
            .filter(|service| !reported_by(*service))
        {
            assert_eq!(read(service, cwd.path()).await.unwrap(), None);
        }
        assert!(!reported_by(Service::Vercel));
    }
}
