use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::git::hidden;

const SIGN_IN: &str = "protocol=https\nhost=github.com\n\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
}

pub struct Repo {
    pub clone_url: String,
    pub html_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullState {
    Open,
    Merged,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub title: String,
    pub date: String,
    pub prerelease: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub state: PullState,
}

fn credential(action: &str, input: &str, interactive: bool) -> Result<String, String> {
    let mut command = hidden("git");
    command.args(["credential", action]).env("GIT_TERMINAL_PROMPT", "0");
    if !interactive {
        command.env("GCM_INTERACTIVE", "never");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Git could not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn token(credential: &str) -> Option<&str> {
    credential.lines().find_map(|line| line.strip_prefix("password="))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::Rustls).root_certs(RootCerts::PlatformVerifier).build())
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn api_error(body: &Value) -> String {
    let detail = body["errors"][0]["message"].as_str().or(body["message"].as_str()).unwrap_or("the request failed");
    format!("GitHub said: {detail}")
}

fn post(path: &str, payload: Value) -> Result<Value, String> {
    let signed_in = credential("fill", SIGN_IN, true)?;
    let token =
        token(&signed_in).ok_or("GitHub sign-in was cancelled. Git's credential manager signs you in the first time you publish.")?;
    let mut response = agent()
        .post(format!("https://api.github.com{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "quill-code")
        .send_json(payload)
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let status = response.status().as_u16();
    let body: Value = response.body_mut().read_json().unwrap_or_default();
    match status {
        200..=299 => {
            _ = credential("approve", &signed_in, false);
            Ok(body)
        }
        401 => {
            _ = credential("reject", &signed_in, false);
            Err("GitHub did not accept the saved sign-in. Try again to sign in afresh.".into())
        }
        _ => Err(api_error(&body)),
    }
}

pub fn create_repo(name: &str, visibility: Visibility) -> Result<Repo, String> {
    let body = post("/user/repos", json!({ "name": name, "private": visibility == Visibility::Private }))?;
    Ok(Repo {
        clone_url: body["clone_url"].as_str().unwrap_or_default().to_string(),
        html_url: body["html_url"].as_str().unwrap_or_default().to_string(),
    })
}

pub fn create_pull_request(slug: &str, head: &str, base: &str, title: &str, body: &str) -> Result<String, String> {
    let created = post(&format!("/repos/{slug}/pulls"), json!({ "title": title, "head": head, "base": base, "body": body }))?;
    Ok(created["html_url"].as_str().unwrap_or_default().to_string())
}

fn parse_pull(pull: &Value) -> Option<PullRequest> {
    let number = pull["number"].as_u64()?;
    let state = if !pull["merged_at"].is_null() {
        PullState::Merged
    } else if pull["state"] == "open" {
        PullState::Open
    } else {
        PullState::Closed
    };
    Some(PullRequest { number, state })
}

pub fn releases(slug: &str) -> Result<Vec<Release>, String> {
    let mut response = agent()
        .get(format!("https://api.github.com/repos/{slug}/releases"))
        .query("per_page", "100")
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "quill-code")
        .call()
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let status = response.status().as_u16();
    let body: Value = response.body_mut().read_json().unwrap_or_default();
    if status != 200 {
        return Err(api_error(&body));
    }
    Ok(body.as_array().into_iter().flatten().filter_map(parse_release).collect())
}

fn parse_release(release: &Value) -> Option<Release> {
    if release["draft"] == true {
        return None;
    }
    Some(Release {
        version: release["tag_name"].as_str()?.trim_start_matches('v').to_string(),
        title: release["name"].as_str().unwrap_or_default().to_string(),
        date: release["published_at"].as_str().unwrap_or_default().chars().take(10).collect(),
        prerelease: release["prerelease"] == true,
    })
}

pub fn pull_request(slug: &str, branch: &str) -> Option<PullRequest> {
    let owner = slug.split('/').next()?;
    let mut request = agent()
        .get(format!("https://api.github.com/repos/{slug}/pulls"))
        .query("head", format!("{owner}:{branch}"))
        .query("state", "all")
        .query("per_page", "1")
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "quill-code");
    let saved = credential("fill", SIGN_IN, false).unwrap_or_default();
    if let Some(token) = token(&saved) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let mut response = request.call().ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let body: Value = response.body_mut().read_json().ok()?;
    parse_pull(&body[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_prefer_the_specific_reason() {
        let taken = json!({ "message": "Repository creation failed.", "errors": [{ "message": "name already exists on this account" }] });
        assert_eq!(api_error(&taken), "GitHub said: name already exists on this account");
        assert_eq!(api_error(&json!({ "message": "Bad credentials" })), "GitHub said: Bad credentials");
        assert_eq!(api_error(&Value::Null), "GitHub said: the request failed");
    }

    #[test]
    fn releases_skip_drafts() {
        let published = json!({ "tag_name": "v1.0.0", "name": "Quill Code v1.0.0", "published_at": "2026-10-16T09:00:00Z", "draft": false, "prerelease": false });
        let draft = json!({ "tag_name": "v1.1.0", "name": "", "published_at": null, "draft": true });
        let release = parse_release(&published).unwrap();
        assert_eq!((release.version.as_str(), release.date.as_str(), release.prerelease), ("1.0.0", "2026-10-16", false));
        assert!(parse_release(&draft).is_none());
    }

    #[test]
    fn pull_requests_report_their_state() {
        let open = json!({ "number": 3, "state": "open", "merged_at": null });
        let merged = json!({ "number": 4, "state": "closed", "merged_at": "2026-10-14T10:00:00Z" });
        let closed = json!({ "number": 5, "state": "closed", "merged_at": null });
        assert_eq!(parse_pull(&open), Some(PullRequest { number: 3, state: PullState::Open }));
        assert_eq!(parse_pull(&merged).map(|p| p.state), Some(PullState::Merged));
        assert_eq!(parse_pull(&closed).map(|p| p.state), Some(PullState::Closed));
        assert_eq!(parse_pull(&Value::Null), None);
    }
}