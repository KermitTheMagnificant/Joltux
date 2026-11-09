//! Talking to Game Jolt's site API.
//!
//! IMPORTANT: this is the same *undocumented* API the official client and
//! website use, not a public/stable API. The endpoint paths below come from
//! the official client's source (github.com/gamejolt/gamejolt). They are
//! unverified against the live service from this environment, so expect to
//! adjust them. Responses are wrapped as `{"payload": {...}}`.

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE, SET_COOKIE};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::models::PackagePayload;

pub const USER_AGENT: &str = concat!("Joltux/", env!("CARGO_PKG_VERSION"), " (unofficial Linux launcher)");
const BASE: &str = "https://gamejolt.com/site-api";

pub struct ApiClient {
    http: Client,
}

pub enum LoginOutcome {
    Authenticated(String),
    CaptchaRequired,
    ApprovalRequired(String),
}

impl ApiClient {
    /// `session_cookie` is the value of the site's `frontend` cookie. It is
    /// optional: public, free games work without it, but anything tied to your
    /// account (owned paid games, keys) needs it.
    pub fn new(session_cookie: Option<&str>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if let Some(cookie) = session_cookie {
            let value = HeaderValue::from_str(&format!("frontend={cookie}"))
                .context("session cookie contains invalid characters")?;
            headers.insert(COOKIE, value);
        }
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { http })
    }

    fn unwrap_payload(body: Value) -> Value {
        body.get("payload").cloned().unwrap_or(body)
    }

    fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{BASE}{path}");
        let resp = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {url} failed"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("not logged in (401). Set your session cookie: joltux config set session_cookie <value>");
        }
        if !status.is_success() {
            bail!("GET {url} returned HTTP {status}");
        }
        Ok(Self::unwrap_payload(resp.json().context("response was not JSON")?))
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{BASE}{path}");
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .with_context(|| format!("POST {url} failed"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("not logged in (401). Run `joltux login` to sign in");
        }
        if !status.is_success() {
            bail!("POST {url} returned HTTP {status}");
        }
        Ok(Self::unwrap_payload(resp.json().context("response was not JSON")?))
    }

    /// Starts a Game Jolt login and returns the session cookie on success.
    pub fn login(&self, username: &str, password: &str) -> Result<LoginOutcome> {
        let url = format!("{BASE}/web/auth/login");
        let resp = self
            .http
            .post(&url)
            .json(&json!({ "username": username, "password": password }))
            .send()
            .context("login request failed")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("login endpoint returned HTTP {status}");
        }
        let cookie = frontend_cookie(resp.headers());
        let payload = Self::unwrap_payload(resp.json().context("login response was not JSON")?);

        if payload.get("success").and_then(Value::as_bool) == Some(true) {
            return cookie
                .map(LoginOutcome::Authenticated)
                .context("login succeeded, but Game Jolt did not return a session cookie");
        }
        if payload.get("token").and_then(Value::as_str).is_some()
            || payload.get("reason").and_then(Value::as_str) == Some("captcha")
        {
            return Ok(LoginOutcome::CaptchaRequired);
        }
        if payload.get("reason").and_then(Value::as_str) == Some("approve-login") {
            let token = payload
                .get("loginPollingToken")
                .and_then(Value::as_str)
                .context("approval response had no login polling token")?;
            return Ok(LoginOutcome::ApprovalRequired(token.to_owned()));
        }

        let reason = payload.get("reason").and_then(Value::as_str).unwrap_or("unknown");
        let message = match reason {
            "invalid-login" => "incorrect username or password".to_owned(),
            "blocked" => "login blocked after too many attempts".to_owned(),
            "approve-login-rejected" => "device approval was rejected".to_owned(),
            "try-again" => "Game Jolt could not process the login; try again later".to_owned(),
            other => format!("Game Jolt rejected the login ({other})"),
        };
        bail!("{message}");
    }

    /// Waits for email approval and returns the session cookie once approved.
    pub fn poll_login_approval(&self, token: &str) -> Result<String> {
        if !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            bail!("approval token contained unsupported characters");
        }
        let url = format!("{BASE}/web/auth/poll-blocked-login/{token}");
        let deadline = Instant::now() + Duration::from_secs(10 * 60);
        loop {
            let resp = self.http.get(&url).send().context("login approval poll failed")?;
            let status = resp.status();
            if !status.is_success() {
                bail!("login approval poll returned HTTP {status}");
            }
            let cookie = frontend_cookie(resp.headers());
            let payload = Self::unwrap_payload(resp.json().context("approval response was not JSON")?);
            match payload.get("status").and_then(Value::as_str) {
                Some("complete") => {
                    return cookie.context("approval completed, but Game Jolt did not return a session cookie");
                }
                Some("error") => match payload.get("reason").and_then(Value::as_str) {
                    Some("expired") => bail!("login approval expired; run `joltux login` again"),
                    Some("rejected") => bail!("device approval was rejected"),
                    _ => bail!("Game Jolt could not approve this login"),
                },
                Some("progress") => {}
                _ => bail!("unexpected response while waiting for login approval"),
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for login approval; run `joltux login` again");
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    }

    /// Game title, if the response has the shape we expect.
    pub fn game_title(&self, game_id: u64) -> Result<Option<String>> {
        let payload = self.get(&format!("/web/discover/games/{game_id}"))?;
        Ok(payload
            .pointer("/game/title")
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// All packages, releases, builds and launch options for a game.
    pub fn packages(&self, game_id: u64) -> Result<PackagePayload> {
        let payload = self.get(&format!("/web/discover/games/packages/{game_id}"))?;
        serde_json::from_value(payload).context("unexpected shape for package payload")
    }

    /// Resolves a build to a short-lived direct download URL.
    pub fn download_url(&self, build_id: u64) -> Result<String> {
        let payload = self.post(
            &format!("/web/discover/games/builds/get-download-url/{build_id}"),
            &json!({ "forceDownload": true }),
        )?;
        payload
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("download-url response had no `url` field")
    }
}

fn frontend_cookie(headers: &HeaderMap) -> Option<String> {
    headers.get_all(SET_COOKIE).iter().find_map(|header| {
        let value = header.to_str().ok()?;
        let (name, value) = value.split_once('=')?;
        if name != "frontend" {
            return None;
        }
        let value = value.split(';').next()?;
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::frontend_cookie;
    use reqwest::header::{HeaderMap, HeaderValue, SET_COOKIE};

    #[test]
    fn extracts_frontend_cookie_without_attributes() {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_static("other=x; Path=/"));
        headers.append(SET_COOKIE, HeaderValue::from_static("frontend=abc123; Path=/; Secure"));
        assert_eq!(frontend_cookie(&headers).as_deref(), Some("abc123"));
    }
}
