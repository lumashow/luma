//! What Settings shows of each CLI service: whether its CLI is installed,
//! who is signed in to it and on what plan, and how much of the plan is
//! used. Everything is read from the CLI, each time it is asked for.

use super::{antigravity, catalog::Service, claude, codex, plan_usage, AgentError};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Provider {
    pub service: Service,
    /// The installed CLI's version. `None` when it is not installed.
    pub version: Option<String>,
    /// Who is signed in. `None` when nobody is.
    pub account: Option<Account>,
    pub usage: Option<plan_usage::PlanUsage>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Account {
    /// The account's email, when the CLI says.
    pub name: Option<String>,
    /// The plan as its vendor names it: `Max 20×`, `ChatGPT Pro`.
    pub plan: Option<String>,
}

/// The services that run an installed CLI, in the picker's order.
pub fn services() -> impl Iterator<Item = Service> {
    Service::ALL
        .into_iter()
        .filter(|service| service.provider().is_none())
}

/// A CLI that has not answered by now is reported as it stands.
const TIMEOUT: Duration = Duration::from_secs(15);

/// `service`'s CLI as it is now. Nothing here fails: a CLI that is missing,
/// signed out or slow is a provider with less to show.
pub async fn read(service: Service, cwd: &Path) -> Provider {
    let version = async {
        let version = match service {
            Service::Claude => claude::version(cwd).await,
            Service::Codex => codex::version(cwd).await,
            Service::Antigravity => antigravity::version(cwd).await,
            Service::OpenRouter | Service::Vercel => None,
        };
        version.as_deref().and_then(version_number)
    };
    let account = async {
        let account = async {
            match service {
                Service::Claude => claude::account(cwd).await,
                Service::Codex => codex::account(cwd).await,
                Service::Antigravity => antigravity::account(cwd).await,
                Service::OpenRouter | Service::Vercel => Ok(None),
            }
        };
        logged(
            service,
            "account",
            flatten(tokio::time::timeout(TIMEOUT, account).await),
        )
    };
    let usage = async { logged(service, "usage", plan_usage::read(service, cwd).await) };
    let (version, account, usage) = tokio::join!(version, account, usage);
    Provider {
        service,
        version,
        // A usage reading is only of the account that is signed in.
        usage: usage.filter(|_| account.is_some()),
        account,
    }
}

fn flatten<T>(
    result: Result<Result<Option<T>, AgentError>, tokio::time::error::Elapsed>,
) -> Result<Option<T>, AgentError> {
    result.unwrap_or_else(|_| Err(AgentError::Invalid("timed out".into())))
}

fn logged<T>(service: Service, what: &str, result: Result<Option<T>, AgentError>) -> Option<T> {
    result.unwrap_or_else(|error| {
        log::warn!("[agent] {} {what} unavailable: {error}", service.label());
        None
    })
}

/// Run `command --version` and return what it printed.
pub(super) async fn version_output(mut command: tokio::process::Command) -> Option<String> {
    command.arg("--version").kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The number in what a CLI prints for `--version`: `2.1.327 (Claude Code)`,
/// `codex-cli 0.163.0`, `1.5.10`.
fn version_number(printed: &str) -> Option<String> {
    printed
        .split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('.'))
        .map(str::to_string)
}

/// A plan's id as its vendor writes the name: `max_20x` is `Max 20×`,
/// `prolite` is `Prolite`.
pub(super) fn plan_name(id: &str) -> String {
    id.split(['_', '-'])
        .filter(|word| !word.is_empty())
        .map(|word| match word.strip_suffix('x') {
            Some(times) if times.parse::<u32>().is_ok() => format!("{times}×"),
            _ => {
                let mut letters = word.chars();
                letters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(letters).collect()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_plans_read_as_their_vendors_write_them() {
        for (printed, number) in [
            ("2.1.327 (Claude Code)", "2.1.327"),
            ("codex-cli 0.163.0", "0.163.0"),
            ("1.5.10", "1.5.10"),
        ] {
            assert_eq!(version_number(printed).as_deref(), Some(number));
        }
        assert_eq!(version_number("command not found"), None);
        assert_eq!(plan_name("max_20x"), "Max 20×");
        assert_eq!(plan_name("prolite"), "Prolite");
        assert_eq!(plan_name("pro"), "Pro");
    }

    /// Only services that run a CLI are providers.
    #[test]
    fn providers_are_the_cli_services() {
        assert!(services().all(|service| service.provider().is_none()));
        assert!(services().count() < Service::ALL.len());
    }
}
