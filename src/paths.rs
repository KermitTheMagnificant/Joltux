//! Where Joltux keeps its files, following the XDG base directory spec
//! (`~/.config/joltux` for settings, `~/.local/share/joltux` for data).

use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

pub struct Paths {
    pub config_file: PathBuf,
    pub library_file: PathBuf,
    pub games_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub prefixes_dir: PathBuf,
    pub runtimes_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "joltux").context("could not determine home directory")?;
        let data = dirs.data_dir();
        Ok(Self {
            config_file: dirs.config_dir().join("config.json"),
            library_file: data.join("library.json"),
            games_dir: data.join("games"),
            downloads_dir: data.join("downloads"),
            prefixes_dir: data.join("prefixes"),
            runtimes_dir: data.join("runtimes"),
            logs_dir: data.join("logs"),
        })
    }

    /// Everything under one folder; used by tests.
    #[cfg(test)]
    pub fn in_dir(root: &std::path::Path) -> Self {
        Self {
            config_file: root.join("config.json"),
            library_file: root.join("library.json"),
            games_dir: root.join("games"),
            downloads_dir: root.join("downloads"),
            prefixes_dir: root.join("prefixes"),
            runtimes_dir: root.join("runtimes"),
            logs_dir: root.join("logs"),
        }
    }

    pub fn game_dir(&self, game_id: u64) -> PathBuf {
        self.games_dir.join(game_id.to_string())
    }

    /// One Wine/Proton prefix per game so one broken game can't affect others.
    /// Output of the game's last launch from the desktop app.
    pub fn log_file(&self, game_id: u64) -> PathBuf {
        self.logs_dir.join(format!("{game_id}.log"))
    }

    pub fn prefix_dir(&self, game_id: u64) -> PathBuf {
        self.prefixes_dir.join(game_id.to_string())
    }
}
