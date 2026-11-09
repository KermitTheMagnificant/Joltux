//! Talking to Game Jolt's site API.
//!
//! IMPORTANT: this is the same *undocumented* API the official client and
//! website use, not a public/stable API. The endpoint paths below come from
//! the official client's source (github.com/gamejolt/gamejolt). They are
//! unverified against the live service from this environment, so expect to
//! adjust them. Responses are wrapped as `{"payload": {...}}`.

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
use serde_json::{json, Value};

use crate::models::PackagePayload;

pub const USER_AGENT: &str = concat!("Joltux/", env!("CARGO_PKG_VERSION"), " (unofficial Linux launcher)");
const BASE: &str = "https://gamejolt.com/site-api";

pub struct ApiClient {
    http: Client,
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
        if !status.is_success() {
            bail!("POST {url} returned HTTP {status}");
        }
        Ok(Self::unwrap_payload(resp.json().context("response was not JSON")?))
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
