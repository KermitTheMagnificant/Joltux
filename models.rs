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

/// Response of `GET /web/search/games`.
#[derive(Debug, Default, Deserialize)]
pub struct GameSearchPayload {
    #[serde(default)]
    pub games: Vec<GameSearchResult>,
    #[serde(default, rename = "gamesCount")]
    pub games_count: u64,
    #[serde(default)]
    pub page: u32,
    #[serde(default, rename = "perPage")]
    pub per_page: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameSearchResult {
    #[serde(deserialize_with = "lenient_u64")]
    pub id: u64,
    #[serde(default, deserialize_with = "lenient_string")]
    pub title: String,
    #[serde(default)]
    pub img_thumbnail: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub has_adult_content: bool,
    #[serde(default)]
    pub developer: Option<GameDeveloper>,
    #[serde(default)]
    pub compatibility: Option<Compatibility>,
    #[serde(default, deserialize_with = "lenient_option_u64")]
    pub follower_count: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameDeveloper {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

impl GameDeveloper {
    pub fn name(&self) -> Option<&str> {
        self.display_name.as_deref().or(self.username.as_deref())
    }
}

/// Which platforms a game has builds for, summarised across all its builds.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Compatibility {
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
    pub type_html: bool,
}

impl Compatibility {
    pub fn linux(&self) -> bool {
        self.os_linux || self.os_linux_64
    }

    pub fn windows(&self) -> bool {
        self.os_windows || self.os_windows_64
    }

    pub fn mac(&self) -> bool {
        self.os_mac || self.os_mac_64
    }
}

/// An uploaded image (header, screenshot, ...) on Game Jolt's media server.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaItem {
    /// A resized copy; the width is a path segment that can be changed.
    #[serde(default)]
    pub mediaserver_url: Option<String>,
}

/// The `game` object of `GET /web/discover/games/:id`.
#[derive(Debug, Clone, Deserialize)]
pub struct GameInfo {
    #[serde(deserialize_with = "lenient_u64")]
    pub id: u64,
    #[serde(default, deserialize_with = "lenient_string")]
    pub title: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub developer: Option<GameDeveloper>,
    #[serde(default)]
    pub compatibility: Option<Compatibility>,
    #[serde(default, deserialize_with = "lenient_option_u64")]
    pub follower_count: Option<u64>,
    #[serde(default, deserialize_with = "lenient_option_u64")]
    pub like_count: Option<u64>,
    #[serde(default, deserialize_with = "lenient_option_u64")]
    pub development_status: Option<u64>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub canceled: bool,
    #[serde(default)]
    pub header_media_item: Option<MediaItem>,
    /// A JSON-encoded rich-text document; see `description_blocks`.
    #[serde(default, deserialize_with = "lenient_string")]
    pub description_content: String,
}

/// A screenshot or video from `GET /web/discover/games/overview/:id`.
#[derive(Debug, Clone, Deserialize)]
pub struct GameMedia {
    /// "image", "video" or "sketchfab".
    #[serde(default, deserialize_with = "lenient_string")]
    pub media_type: String,
    /// For videos, the provider ("youtube", "vimeo").
    #[serde(rename = "type", default, deserialize_with = "lenient_string")]
    pub provider: String,
    /// For videos, the provider's video ID.
    #[serde(default, deserialize_with = "lenient_string")]
    pub url: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub title: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub caption: String,
    #[serde(default)]
    pub img_thumbnail: Option<String>,
    #[serde(default)]
    pub media_item: Option<MediaItem>,
}

impl GameMedia {
    /// Link to watch a video on its provider's site, if the ID looks sane.
    pub fn video_link(&self) -> Option<String> {
        if self.media_type != "video"
            || self.url.is_empty()
            || !self.url.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
        match self.provider.as_str() {
            "youtube" => Some(format!("https://www.youtube.com/watch?v={}", self.url)),
            "vimeo" => Some(format!("https://vimeo.com/{}", self.url)),
            _ => None,
        }
    }
}

/// Everything shown on a game's preview page.
#[derive(Debug, Clone)]
pub struct GameDetails {
    pub game: GameInfo,
    pub media: Vec<GameMedia>,
    pub meta_description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptionBlock {
    Heading(String),
    Paragraph(String),
    ListItem(String),
}

/// Flattens a game's rich-text description into headings, paragraphs and
/// list items. Images, embeds and formatting are dropped.
pub fn description_blocks(content: &str) -> Vec<DescriptionBlock> {
    fn inline_text(node: &Value, out: &mut String) {
        match node.get("type").and_then(Value::as_str) {
            Some("text") => out.push_str(node.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("hardBreak") => out.push('\n'),
            Some("mention") | Some("tag") => {
                if let Some(text) = node.pointer("/attrs/value").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            _ => {}
        }
        for child in node.get("content").and_then(Value::as_array).into_iter().flatten() {
            inline_text(child, out);
        }
    }

    fn walk(node: &Value, out: &mut Vec<DescriptionBlock>) {
        let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
        let text = || {
            let mut text = String::new();
            inline_text(node, &mut text);
            text.trim().to_owned()
        };
        match kind {
            "heading" => {
                let text = text();
                if !text.is_empty() {
                    out.push(DescriptionBlock::Heading(text));
                }
            }
            "paragraph" | "codeBlock" => {
                let text = text();
                if !text.is_empty() {
                    out.push(DescriptionBlock::Paragraph(text));
                }
            }
            "listItem" => {
                let text = text();
                if !text.is_empty() {
                    out.push(DescriptionBlock::ListItem(text));
                }
            }
            _ => {
                for child in node.get("content").and_then(Value::as_array).into_iter().flatten() {
                    walk(child, out);
                }
            }
        }
    }

    let mut blocks = Vec::new();
    if let Ok(doc) = serde_json::from_str::<Value>(content) {
        walk(&doc, &mut blocks);
    }
    blocks
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
    use super::{description_blocks, DescriptionBlock, GameMedia, GameSearchPayload, PackagePayload};

    #[test]
    fn flattens_rich_text_description() {
        let doc = r#"{"version":"4","content":[
            {"type":"heading","content":[{"type":"text","text":"Features"}]},
            {"type":"paragraph","content":[
                {"type":"text","text":"Line one"},{"type":"hardBreak"},
                {"type":"text","marks":[{"type":"strong"}],"text":"line two"}]},
            {"type":"paragraph","content":[{"type":"gif"}]},
            {"type":"bulletList","content":[{"type":"listItem","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Fast"}]}]}]}
        ]}"#;
        assert_eq!(
            description_blocks(doc),
            vec![
                DescriptionBlock::Heading("Features".into()),
                DescriptionBlock::Paragraph("Line one\nline two".into()),
                DescriptionBlock::ListItem("Fast".into()),
            ]
        );
        assert!(description_blocks("not json").is_empty());
    }

    #[test]
    fn builds_video_links_only_for_safe_ids() {
        let video = |provider: &str, url: &str| GameMedia {
            media_type: "video".into(),
            provider: provider.into(),
            url: url.into(),
            title: String::new(),
            caption: String::new(),
            img_thumbnail: None,
            media_item: None,
        };
        assert_eq!(
            video("youtube", "R8Ud2e-LB20").video_link().as_deref(),
            Some("https://www.youtube.com/watch?v=R8Ud2e-LB20")
        );
        assert_eq!(video("youtube", "x&list=1").video_link(), None);
        assert_eq!(video("other", "abc").video_link(), None);
    }

    #[test]
    fn parses_game_search_results_and_pagination() {
        let payload: GameSearchPayload = serde_json::from_str(
            r#"{
                "games": [{
                    "id": 123,
                    "title": "Sample game",
                    "slug": "sample-game",
                    "img_thumbnail": "https://example.test/thumbnail.webp",
                    "developer": {"display_name": "Studio", "username": "studio"},
                    "unmodeled_field": true
                }],
                "gamesCount": 4518,
                "page": 2,
                "perPage": 36
            }"#,
        )
        .unwrap();

        assert_eq!(payload.games_count, 4518);
        assert_eq!(payload.page, 2);
        assert_eq!(payload.per_page, 36);
        assert_eq!(payload.games[0].id, 123);
        assert_eq!(payload.games[0].title, "Sample game");
        assert_eq!(
            payload.games[0]
                .developer
                .as_ref()
                .and_then(|developer| developer.display_name.as_deref()),
            Some("Studio")
        );
    }

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
