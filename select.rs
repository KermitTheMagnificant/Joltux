//! Deciding which build of a game to install.
//!
//! Policy: prefer a native Linux build (fast, no compatibility layer). Fall
//! back to a Windows build that will run through Proton/Wine. Browser builds,
//! installers, removed builds and macOS-only builds are never chosen.

use serde::{Deserialize, Serialize};

use crate::models::{Build, LaunchOption, PackagePayload, Release};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Linux,
    Windows,
}

#[derive(Debug)]
pub struct Choice<'a> {
    pub build: &'a Build,
    pub release: Option<&'a Release>,
    pub platform: Platform,
    /// Lower is better. Linux 64 < Linux 32 < Windows 64 < Windows 32.
    pub rank: u8,
}

/// Best (platform, rank) a build offers, or `None` if it only targets
/// something we can't run (macOS, web, ...).
fn classify(build: &Build, windows_only: bool) -> Option<(Platform, u8)> {
    if !windows_only {
        if build.os_linux_64 {
            return Some((Platform::Linux, 0));
        }
        if build.os_linux {
            return Some((Platform::Linux, 1));
        }
    }
    if build.os_windows_64 {
        return Some((Platform::Windows, 2));
    }
    if build.os_windows {
        return Some((Platform::Windows, 3));
    }
    None
}

/// Every installable build, best first. With `windows_only` set, native Linux
/// builds are ignored (useful when the Linux build is broken).
pub fn candidates(payload: &PackagePayload, windows_only: bool) -> Vec<Choice<'_>> {
    let mut out: Vec<Choice<'_>> = payload
        .builds
        .iter()
        .filter(|b| b.is_downloadable() && b.is_active() && !b.is_installer)
        .filter_map(|build| {
            let (platform, rank) = classify(build, windows_only)?;
            let release = payload.releases.iter().find(|r| r.id == build.game_release_id);
            Some(Choice { build, release, platform, rank })
        })
        .collect();

    // Sort by rank first, then by the release's own `sort` value.
    out.sort_by_key(|c| (c.rank, c.release.map(|r| r.sort).unwrap_or(i64::MAX)));
    out
}

pub fn choose(payload: &PackagePayload, windows_only: bool) -> Option<Choice<'_>> {
    candidates(payload, windows_only).into_iter().next()
}

/// Find the launch option (which file to run) for a chosen build.
pub fn launch_option<'a>(
    options: &'a [LaunchOption],
    build_id: u64,
    platform: Platform,
) -> Option<&'a LaunchOption> {
    let wanted: &[&str] = match platform {
        Platform::Linux => &["linux_64", "linux"],
        Platform::Windows => &["windows_64", "windows"],
    };
    let mine = || options.iter().filter(|o| o.game_build_id == build_id);

    for os in wanted {
        if let Some(found) = mine().find(|o| o.os.as_deref() == Some(*os)) {
            return Some(found);
        }
    }
    // An option with no OS applies to everything.
    mine().find(|o| o.os.as_deref().map(str::is_empty).unwrap_or(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PackagePayload {
        // Mixed booleans/ints/strings on purpose: the API isn't consistent.
        serde_json::from_str(
            r#"{
              "packages": [{"id": 1, "title": "Game"}],
              "releases": [{"id": 10, "game_package_id": 1, "version_number": "1.0", "sort": 1}],
              "builds": [
                {"id": 100, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "active", "os_windows_64": 1},
                {"id": 101, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "active", "os_linux_64": "1"},
                {"id": 102, "game_package_id": 1, "game_release_id": 10, "type": "html",
                 "status": "active"},
                {"id": 103, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "removed", "os_linux_64": true}
              ],
              "launchOptions": [
                {"game_build_id": 100, "os": "windows_64", "executable_path": "Game.exe"},
                {"game_build_id": 101, "os": "linux_64", "executable_path": "game.x86_64"}
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn prefers_native_linux() {
        let p = sample();
        let c = choose(&p, false).unwrap();
        assert_eq!(c.build.id, 101);
        assert_eq!(c.platform, Platform::Linux);
    }

    #[test]
    fn windows_only_skips_linux() {
        let p = sample();
        let c = choose(&p, true).unwrap();
        assert_eq!(c.build.id, 100);
        assert_eq!(c.platform, Platform::Windows);
    }

    #[test]
    fn skips_html_and_removed_builds() {
        let p = sample();
        let ids: Vec<u64> = candidates(&p, false).iter().map(|c| c.build.id).collect();
        assert_eq!(ids, vec![101, 100]);
    }

    #[test]
    fn finds_launch_option() {
        let p = sample();
        let lo = launch_option(&p.launch_options, 100, Platform::Windows).unwrap();
        assert_eq!(lo.executable_path, "Game.exe");
        assert!(launch_option(&p.launch_options, 100, Platform::Linux).is_none());
    }
}
