//! A CLI engine whose login is missing or has expired.
//!
//! The CLIs own their logins, so Luma neither stores nor refreshes one. What
//! it does is recognize the failure and run the CLI's own sign-in: each opens
//! the browser itself and exits, or answers, once the login is saved.
//!
//! A turn's failure reaches a host as text, so the failure is recognized in
//! the text ([`signed_out`]). The engines word the cases they can tell apart
//! structurally in the constants here; what a CLI words itself is matched by
//! the phrases the CLIs are known to use.

use super::{antigravity, catalog::Service, claude, codex, protocol, AgentError};
use std::time::Duration;

pub(super) const SIGNED_OUT_CLAUDE: &str =
    "Claude Code isn't signed in with a Claude subscription. Sign in, then send again.";
pub(super) const SIGNED_OUT_CODEX: &str =
    "Codex isn't signed in with ChatGPT. Sign in, then send again.";
pub(super) const SIGNED_OUT_ANTIGRAVITY: &str =
    "Antigravity isn't signed in. Sign in, then send again.";

// Adapted from Zeron, MIT, (c) 2026 Wing: `run_errors::provider_problem`.
const SIGNED_OUT: &[&str] = &[
    "isn't signed in",
    "is not signed in",
    "not signed in",
    "not logged in",
    "please run /login",
    "authentication failed",
    "authentication_failed",
    "authentication_error",
    "authentication required",
    "sign in again",
    "log in again",
    "invalid api key",
    "invalid x-api-key",
    "oauth token has expired",
    "token has expired",
    "token expired",
    "refresh token",
    "unauthorized",
    "http 401",
    "status 401",
    "(401)",
    "logged out",
];

/// Whether `message`, a turn's failure, says the engine's CLI needs a
/// sign-in.
pub fn signed_out(message: &str) -> bool {
    let text = message.to_ascii_lowercase();
    SIGNED_OUT.iter().any(|phrase| text.contains(phrase))
}

/// How long a sign-in may wait on the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

/// Run `service`'s own sign-in and wait for it. Dropping the future stops
/// the CLI.
///
/// # Errors
///
/// When the service has no CLI login, the CLI is missing, the person did not
/// finish in the browser in time, or the CLI reports the sign-in failed.
pub async fn sign_in(service: Service, cwd: &std::path::Path) -> Result<(), AgentError> {
    let work = async {
        match service {
            Service::Claude => finished(claude::sign_in_command(cwd)?, service).await,
            Service::Codex => finished(codex::sign_in_command(cwd)?, service).await,
            Service::Antigravity => antigravity::sign_in(cwd).await,
            Service::OpenRouter | Service::Vercel => Err(AgentError::Invalid(format!(
                "{} has no sign-in; set its API key in Settings",
                service.label()
            ))),
        }
    };
    tokio::time::timeout(SIGN_IN_TIMEOUT, work)
        .await
        .map_err(|_| protocol(format!("{} sign-in was not finished", service.label())))?
}

/// Run a CLI's login command to its end.
async fn finished(
    mut command: tokio::process::Command,
    service: Service,
) -> Result<(), AgentError> {
    let output = command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| protocol(format!("{} sign-in: {error}", service.label())))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.lines().rev().find(|line| !line.trim().is_empty());
    Err(protocol(format!(
        "{} sign-in failed: {}",
        service.label(),
        said.map_or_else(|| output.status.to_string(), |line| line.trim().to_string())
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the engines say when a login is missing, and what the CLIs say
    /// themselves, is a sign-in; an ordinary failure is not.
    #[test]
    fn a_missing_login_is_told_from_other_failures() {
        for message in [
            SIGNED_OUT_CLAUDE,
            SIGNED_OUT_CODEX,
            SIGNED_OUT_ANTIGRAVITY,
            "Claude: Not logged in · Please run /login",
            "Claude: authentication_failed",
            "Claude: Invalid API key · Fix external API key",
            "Antigravity: Authentication required",
            "Codex: unexpected status 401 Unauthorized",
        ] {
            assert!(signed_out(message), "{message}");
        }
        for message in [
            "Claude: Request rejected: try a new session.",
            "Codex turn did not complete: interrupted",
            "Antigravity: Unknown model ID: carbon",
            "Effort high is not supported by the selected model",
        ] {
            assert!(!signed_out(message), "{message}");
        }
    }

    #[tokio::test]
    async fn a_gateway_has_no_sign_in() {
        let cwd = tempfile::tempdir().unwrap();
        assert!(sign_in(Service::Vercel, cwd.path()).await.is_err());
    }
}
