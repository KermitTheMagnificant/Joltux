//! Checking installed games for newer builds.
//!
//! An update stays on the package (e.g. "Demo") and platform the game was
//! installed from, and only counts if it's a different release: a 32-bit
//! build of the same version is not an update.

use std::collections::BTreeSet;

use anyhow::{bail, Result};

use crate::api::ApiClient;
use crate::models::{InstalledGame, PackagePayload};
use crate::select::{self, Filter, Platform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub build_id: u64,
    pub package_id: u64,
    pub version: String,
}

/// Asks Game Jolt whether `game` has a newer build.
pub fn check(api: &ApiClient, game: &InstalledGame) -> Result<Option<Update>> {
    find(&api.packages(game.game_id)?, game)
}

/// The best newer build of `game` in `payload`, if there is one.
pub fn find(payload: &PackagePayload, game: &InstalledGame) -> Result<Option<Update>> {
    let installed = payload.builds.iter().find(|b| b.id == game.build_id);
    let package_id = match game.package_id.or(installed.map(|b| b.game_package_id)) {
        Some(id) => id,
        None => only_package(payload, game.platform)?,
    };
    let filter = Filter {
        windows_only: game.platform == Platform::Windows,
        package_id: Some(package_id),
        build_id: None,
    };
    let Some(best) = select::candidates(payload, &filter)
        .into_iter()
        .find(|c| c.platform == game.platform)
    else {
        return Ok(None);
    };
    let same_release = installed.is_some_and(|b| b.game_release_id == best.build.game_release_id);
    if best.build.id == game.build_id || same_release {
        return Ok(None);
    }
    Ok(Some(Update {
        build_id: best.build.id,
        package_id,
        version: best.release.map(|r| r.version_number.clone()).unwrap_or_default(),
    }))
}

/// For games installed before Joltux recorded packages, whose old build is
/// gone: the package is only known if exactly one offers this platform.
fn only_package(payload: &PackagePayload, platform: Platform) -> Result<u64> {
    let filter = Filter {
        windows_only: platform == Platform::Windows,
        ..Default::default()
    };
    let packages: BTreeSet<u64> = select::candidates(payload, &filter)
        .iter()
        .filter(|c| c.platform == platform)
        .map(|c| c.build.game_package_id)
        .collect();
    match packages.len() {
        1 => Ok(*packages.first().expect("one package")),
        0 => Ok(0), // nothing installable; `find` then reports no update
        _ => bail!("can't tell which package is installed; reinstall it with --package once to fix this"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LaunchSettings;
    use std::path::PathBuf;

    fn payload() -> PackagePayload {
        serde_json::from_str(
            r#"{
              "packages": [{"id": 1, "title": "Full Game"}, {"id": 2, "title": "Demo"}],
              "releases": [
                {"id": 10, "version_number": "1.0", "sort": 1},
                {"id": 11, "version_number": "1.1", "sort": 0},
                {"id": 20, "version_number": "0.2", "sort": 0}
              ],
              "builds": [
                {"id": 100, "game_package_id": 1, "game_release_id": 10, "type": "downloadable", "os_linux_64": true},
                {"id": 101, "game_package_id": 1, "game_release_id": 10, "type": "downloadable", "os_linux": true},
                {"id": 110, "game_package_id": 1, "game_release_id": 11, "type": "downloadable", "os_linux_64": true},
                {"id": 111, "game_package_id": 1, "game_release_id": 11, "type": "downloadable", "os_windows_64": true},
                {"id": 200, "game_package_id": 2, "game_release_id": 20, "type": "downloadable", "os_linux_64": true}
              ]
            }"#,
        )
        .unwrap()
    }

    fn installed(build_id: u64, platform: Platform, package_id: Option<u64>) -> InstalledGame {
        InstalledGame {
            game_id: 5,
            title: "Game".into(),
            build_id,
            platform,
            version: String::new(),
            install_dir: PathBuf::new(),
            executable: PathBuf::new(),
            package_id,
            launch: LaunchSettings::default(),
        }
    }

    #[test]
    fn finds_newer_release_on_same_platform() {
        let update = find(&payload(), &installed(100, Platform::Linux, None)).unwrap().unwrap();
        assert_eq!(update, Update { build_id: 110, package_id: 1, version: "1.1".into() });

        let update = find(&payload(), &installed(999, Platform::Windows, Some(1))).unwrap().unwrap();
        assert_eq!(update.build_id, 111);
    }

    #[test]
    fn up_to_date_and_same_release_are_not_updates() {
        assert_eq!(find(&payload(), &installed(110, Platform::Linux, Some(1))).unwrap(), None);
        // 32-bit build of the newest release: same release, so no update.
        let mut p = payload();
        p.builds.push(serde_json::from_str(
            r#"{"id": 112, "game_package_id": 1, "game_release_id": 11, "type": "downloadable", "os_linux": true}"#,
        ).unwrap());
        assert_eq!(find(&p, &installed(112, Platform::Linux, Some(1))).unwrap(), None);
    }

    #[test]
    fn stays_on_installed_package() {
        assert_eq!(find(&payload(), &installed(200, Platform::Linux, None)).unwrap(), None);
    }

    #[test]
    fn unknown_package_with_removed_build() {
        // Old library entry, its build is gone and two packages offer Linux.
        assert!(find(&payload(), &installed(999, Platform::Linux, None)).is_err());
        // Only package 1 offers Windows, so that one is assumed.
        assert_eq!(find(&payload(), &installed(999, Platform::Windows, None)).unwrap().unwrap().build_id, 111);
    }
}
