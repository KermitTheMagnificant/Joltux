//! Data types for the parts of Game Jolt's site API that we use.
//!
//! These mirror the models in the official client's source
//! (`src/_common/game/build/build.model.ts` etc). Every field except `id`
//! has a default so a missing or renamed field degrades gracefully instead of
//! failing the whole response.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// The site's PHP backend isn't consistent about booleans (the official client
/// writes `!!build.os_windows`, which hints at truthy non-booleans). This
/// accepts `true`/`false`, `0`/`1`, `"0"`/`"1"` and `null`.
fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_i64().map(|i| i != 0).unwrap_or(false),
        Value::String(s) => matches!(s.as_str(), "1" | "true"),
        _ => false,
    })
}

/// Response of `GET /web/discover/games/packages/:gameId`.
#[derive(Debug, Default, Deserialize)]
pub struct PackagePayload {
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub releases: Vec<Release>,
    #[serde(default)]
    pub builds: Vec<Build>,
    #[serde(default, rename = "launchOptions")]
    pub launch_options: Vec<LaunchOption>,
}

/// A "package" is a named group of releases (e.g. "Demo", "Full game").
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub id: u64,
    #[serde(default)]
    pub game_package_id: u64,
    #[serde(default)]
    pub version_number: String,
    /// The official client sorts releases by this ascending and takes the
    /// first match, so lower = preferred.
    #[serde(default)]
    pub sort: i64,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Build {
    pub id: u64,
    #[serde(default)]
    pub game_package_id: u64,
    #[serde(default)]
    pub game_release_id: u64,
    /// "downloadable", "html", "flash", "unity", "applet", "rom", ...
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub archive_type: String,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub os_windows: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub os_windows_64: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub os_mac: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub os_mac_64: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub os_linux: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub os_linux_64: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_installer: bool,
    #[serde(default)]
    pub primary_file: Option<BuildFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BuildFile {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub filesize: Option<u64>,
}

/// Tells the launcher which file inside the archive to run, per OS.
#[derive(Debug, Clone, Deserialize)]
pub struct LaunchOption {
    #[serde(default)]
    pub game_build_id: u64,
    /// "windows", "windows_64", "linux", "linux_64", "mac", "mac_64" or empty.
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub executable_path: String,
}

impl Build {
    pub fn is_downloadable(&self) -> bool {
        self.kind == "downloadable"
    }

    /// A missing status is treated as active; only explicitly removed or
    /// still-processing builds are skipped.
    pub fn is_active(&self) -> bool {
        self.status.is_empty() || self.status == "active"
    }
}

/// What we remember about an installed game (stored in library.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledGame {
    pub game_id: u64,
    pub title: String,
    pub build_id: u64,
    pub platform: crate::select::Platform,
    pub version: String,
    pub install_dir: std::path::PathBuf,
    pub executable: std::path::PathBuf,
}
