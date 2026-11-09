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

fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let value = Value::deserialize(d)?;
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        Value::String(number) => number.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom(
            "expected an unsigned integer or numeric string",
        )),
    }
}

fn lenient_option_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let value = Value::deserialize(d)?;
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        Value::String(number) => number.parse().map(Some).map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom(
            "expected an unsigned integer, numeric string, or null",
        )),
    }
}

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = Value::deserialize(d)?;
    match value {
        Value::String(value) => Ok(value),
        Value::Null => Ok(String::new()),
        _ => Err(serde::de::Error::custom("expected a string or null")),
    }
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
    #[serde(deserialize_with = "lenient_u64")]
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    #[serde(deserialize_with = "lenient_u64")]
    pub id: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub game_package_id: u64,
    #[serde(default, deserialize_with = "lenient_string")]
    pub version_number: String,
    /// The official client sorts releases by this ascending and takes the
    /// first match, so lower = preferred.
    #[serde(default)]
    pub sort: i64,
    #[serde(default, deserialize_with = "lenient_string")]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Build {
    #[serde(deserialize_with = "lenient_u64")]
    pub id: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub game_package_id: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub game_release_id: u64,
    /// "downloadable", "html", "flash", "unity", "applet", "rom", ...
    #[serde(rename = "type", default, deserialize_with = "lenient_string")]
    pub kind: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub status: String,
    #[serde(default, deserialize_with = "lenient_string")]
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
    #[serde(default, deserialize_with = "lenient_option_u64")]
    pub filesize: Option<u64>,
    #[serde(default)]
    pub is_archive: Option<bool>,
}

/// Tells the launcher which file inside the archive to run, per OS.
#[derive(Debug, Clone, Deserialize)]
pub struct LaunchOption {
    #[serde(default, deserialize_with = "lenient_u64")]
    pub game_build_id: u64,
    /// "windows", "windows_64", "linux", "linux_64", "mac", "mac_64" or empty.
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
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

#[cfg(test)]
mod tests {
    use super::PackagePayload;

    #[test]
    fn parses_numeric_strings_in_package_payload() {
        let payload: PackagePayload = serde_json::from_str(
                        r#"{
                            "packages": [{"id": "258246645", "title": "Game"}],
                            "releases": [{
                                "id": "10", "game_package_id": "258246645",
                                "version_number": null, "status": null
                            }],
                            "builds": [{
                                "id": "100", "game_package_id": "258246645", "game_release_id": "10",
                                "type": null, "status": null, "archive_type": null,
                                "primary_file": {"filesize": "4096", "is_archive": true}
                            }],
                            "launchOptions": [{"game_build_id": "100", "executable_path": null}]
                        }"#,
                )
                .unwrap();

        assert_eq!(payload.packages[0].id, 258246645);
        assert_eq!(payload.releases[0].game_package_id, 258246645);
        assert_eq!(payload.builds[0].game_release_id, 10);
        assert_eq!(
            payload.builds[0].primary_file.as_ref().unwrap().filesize,
            Some(4096)
        );
        assert_eq!(payload.launch_options[0].game_build_id, 100);
        assert_eq!(payload.builds[0].kind, "");
        assert_eq!(payload.launch_options[0].executable_path, "");
    }
}
