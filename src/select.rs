//! Deciding which build of a game to install.
//!
//! Policy: prefer a native Linux build (fast, no compatibility layer). Fall
//! back to a Windows build that will run through Proton/Wine. Browser builds,
//! non-executable raw files, installers, removed builds, hidden/removed
//! releases and macOS-only builds are never chosen.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::models::{Build, LaunchOption, Package, PackagePayload, Release};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Linux,
    Windows,
}

#[derive(Debug)]
pub struct Choice<'a> {
    pub build: &'a Build,
    pub release: Option<&'a Release>,
    pub package: Option<&'a Package>,
    pub platform: Platform,
    /// Lower is better. Linux 64 < Linux 32 < Windows 64 < Windows 32.
    pub rank: u8,
}

impl Choice<'_> {
    pub fn package_title(&self) -> String {
        match self.package {
            Some(p) => p.display_title(),
            None => format!("package {}", self.build.game_package_id),
        }
    }
}

/// Narrows down which builds are considered.
#[derive(Debug, Default, Clone, Copy)]
pub struct Filter {
    /// Ignore native Linux builds (useful when the Linux build is broken).
    pub windows_only: bool,
    /// Only builds from this package.
    pub package_id: Option<u64>,
    /// Only this exact build.
    pub build_id: Option<u64>,
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

pub fn is_standalone_windows_executable(build: &Build) -> bool {
    (build.os_windows || build.os_windows_64)
        && build
            .primary_file
            .as_ref()
            .and_then(|file| file.filename.as_deref())
            .is_some_and(|filename| filename.to_ascii_lowercase().ends_with(".exe"))
}

/// Every installable build that passes `filter`, best first.
pub fn candidates<'a>(payload: &'a PackagePayload, filter: &Filter) -> Vec<Choice<'a>> {
    let mut out: Vec<Choice<'a>> = payload
        .builds
        .iter()
        .filter(|b| filter.build_id.is_none_or(|id| b.id == id))
        .filter(|b| filter.package_id.is_none_or(|id| b.game_package_id == id))
        .filter_map(|build| {
            if !build.is_downloadable() || !build.is_active() || build.is_installer {
                return None;
            }
            let (platform, rank) = classify(build, filter.windows_only)?;
            let is_standalone_exe = platform == Platform::Windows
                && build.primary_file.as_ref().and_then(|file| file.is_archive) == Some(false)
                && is_standalone_windows_executable(build);
            if build.primary_file.as_ref().and_then(|file| file.is_archive) == Some(false)
                && !is_standalone_exe
            {
                return None;
            }
            let release = payload
                .releases
                .iter()
                .find(|r| r.id == build.game_release_id);
            if release.is_some_and(|r| !r.is_published()) {
                return None;
            }
            let package = payload.packages.iter().find(|p| p.id == build.game_package_id);
            Some(Choice {
                build,
                release,
                package,
                platform,
                rank,
            })
        })
        .collect();

    // Sort by rank first, then by the release's own `sort` value.
    out.sort_by_key(|c| (c.rank, c.release.map(|r| r.sort).unwrap_or(i64::MAX)));
    out
}

pub fn choose<'a>(payload: &'a PackagePayload, filter: &Filter) -> Option<Choice<'a>> {
    candidates(payload, filter).into_iter().next()
}

/// Resolves `--package` input: a package ID, an exact title, or a unique
/// part of a title (all case-insensitive).
pub fn find_package<'a>(payload: &'a PackagePayload, query: &str) -> Result<&'a Package> {
    let q = query.trim().to_lowercase();
    if let Ok(id) = q.parse::<u64>() {
        if let Some(p) = payload.packages.iter().find(|p| p.id == id) {
            return Ok(p);
        }
    }
    if let Some(p) = payload.packages.iter().find(|p| p.display_title().to_lowercase() == q) {
        return Ok(p);
    }
    let partial: Vec<&Package> = payload
        .packages
        .iter()
        .filter(|p| p.display_title().to_lowercase().contains(&q))
        .collect();
    let names = || {
        payload
            .packages
            .iter()
            .map(|p| format!("\"{}\" ({})", p.display_title(), p.id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match partial.as_slice() {
        [one] => Ok(one),
        [] => bail!("no package matches `{query}`; available: {}", names()),
        _ => bail!("`{query}` matches several packages; be more specific. Available: {}", names()),
    }
}

/// Find the launch option (which file to run) for a chosen build.
pub fn launch_option(
    options: &[LaunchOption],
    build_id: u64,
    platform: Platform,
) -> Option<&LaunchOption> {
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
              "packages": [{"id": 1, "title": "Full Game"}, {"id": 2, "title": "Demo"}],
              "releases": [
                {"id": 10, "game_package_id": 1, "version_number": "1.0", "sort": 1, "status": "published"},
                {"id": 11, "game_package_id": 1, "version_number": "0.9", "sort": 0, "status": "hidden"},
                {"id": 20, "game_package_id": 2, "version_number": "0.1", "sort": 0}
              ],
              "builds": [
                {"id": 100, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "active", "os_windows_64": 1},
                {"id": 101, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "active", "os_linux_64": "1"},
                {"id": 102, "game_package_id": 1, "game_release_id": 10, "type": "html",
                 "status": "active"},
                {"id": 103, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "removed", "os_linux_64": true},
                {"id": 104, "game_package_id": 1, "game_release_id": 10, "type": "downloadable",
                 "status": "active", "os_windows_64": true,
                 "primary_file": {"filename": "game.bin", "is_archive": false}},
                {"id": 105, "game_package_id": 1, "game_release_id": 11, "type": "downloadable",
                 "status": "active", "os_linux_64": true},
                {"id": 200, "game_package_id": 2, "game_release_id": 20, "type": "downloadable",
                 "status": "active", "os_windows": true}
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
        let c = choose(&p, &Filter::default()).unwrap();
        assert_eq!(c.build.id, 101);
        assert_eq!(c.platform, Platform::Linux);
    }

    #[test]
    fn windows_only_skips_linux() {
        let p = sample();
        let c = choose(&p, &Filter { windows_only: true, ..Default::default() }).unwrap();
        assert_eq!(c.build.id, 100);
        assert_eq!(c.platform, Platform::Windows);
    }

    #[test]
    fn skips_html_and_removed_builds() {
        let p = sample();
        let ids: Vec<u64> = candidates(&p, &Filter::default()).iter().map(|c| c.build.id).collect();
        // 102 is html, 103 is removed, 104 is a raw non-exe file,
        // 105 belongs to a hidden release.
        assert_eq!(ids, vec![101, 100, 200]);
    }

    #[test]
    fn filters_by_package_and_build() {
        let p = sample();
        let demo = find_package(&p, "demo").unwrap();
        assert_eq!(demo.id, 2);
        let c = choose(&p, &Filter { package_id: Some(demo.id), ..Default::default() }).unwrap();
        assert_eq!(c.build.id, 200);
        assert_eq!(c.package_title(), "Demo");

        let c = choose(&p, &Filter { build_id: Some(100), ..Default::default() }).unwrap();
        assert_eq!(c.build.id, 100);
        assert!(choose(&p, &Filter { build_id: Some(105), ..Default::default() }).is_none());
    }

    #[test]
    fn package_lookup_by_id_title_and_ambiguity() {
        let p = sample();
        assert_eq!(find_package(&p, "1").unwrap().id, 1);
        assert_eq!(find_package(&p, "FULL game").unwrap().id, 1);
        assert!(find_package(&p, "nope").is_err());
        // "e" is in both "Full Game" and "Demo".
        assert!(find_package(&p, "e").is_err());
    }

    #[test]
    fn finds_launch_option() {
        let p = sample();
        let lo = launch_option(&p.launch_options, 100, Platform::Windows).unwrap();
        assert_eq!(lo.executable_path, "Game.exe");
        assert!(launch_option(&p.launch_options, 100, Platform::Linux).is_none());
    }

        #[test]
        fn accepts_standalone_windows_executable_without_launch_path() {
                let payload: PackagePayload = serde_json::from_str(
                        r#"{
                            "builds": [{
                                "id": 731959, "type": "downloadable", "status": "active",
                                "os_windows": true, "os_windows_64": true, "is_installer": false,
                                "primary_file": {
                                    "filename": "five-nights-at-candy-s-remastered.exe",
                                    "is_archive": false
                                }
                            }],
                            "launchOptions": [{"game_build_id": 731959, "os": null, "executable_path": null}]
                        }"#,
                )
                .unwrap();

                let choice = choose(&payload, &Filter::default()).unwrap();
                assert_eq!(choice.build.id, 731959);
                assert_eq!(choice.platform, Platform::Windows);
                assert!(is_standalone_windows_executable(choice.build));
        }
}
