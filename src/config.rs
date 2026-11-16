//! User settings, stored as JSON in the config directory.

use std::fs;
use std::io::Write;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// umu-launcher: runs Proton outside Steam (recommended).
    Umu,
    /// Plain `wine` from your distro.
    Wine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub runner: Runner,
    /// Passed to umu as PROTONPATH: a Proton folder path, or a name such as
    /// "GE-Proton" (umu downloads the latest GE-Proton for you).
    pub proton_path: String,
    /// Value of the website's `frontend` cookie. Stored in plain text, so the
    /// file is created with owner-only permissions.
    pub session_cookie: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            runner: Runner::Umu,
            proton_path: "GE-Proton".to_string(),
            session_cookie: None,
        }
    }
}

impl Config {
    /// A missing file just means "use defaults"; a corrupt one is an error.
    pub fn load(paths: &Paths) -> Result<Self> {
        match fs::read_to_string(&paths.config_file) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", paths.config_file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("could not read config file"),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        if let Some(parent) = paths.config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        // The file may hold the session cookie, so it's created owner-only
        // from the start (no window where it's world-readable), written to a
        // temp file and renamed over the old one. Renaming also replaces any
        // older config that had looser permissions.
        let tmp = paths.config_file.with_extension("json.tmp");
        let _ = fs::remove_file(&tmp);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &paths.config_file).context("could not save config file")?;
        Ok(())
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "runner" => {
                self.runner = match value {
                    "umu" => Runner::Umu,
                    "wine" => Runner::Wine,
                    other => bail!("runner must be `umu` or `wine`, not `{other}`"),
                }
            }
            "proton_path" => self.proton_path = value.to_string(),
            "session_cookie" => {
                self.session_cookie = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            }
            other => bail!("unknown setting `{other}` (try: runner, proton_path, session_cookie)"),
        }
        Ok(())
    }

    /// Settings for display, with the cookie hidden.
    pub fn redacted(&self) -> String {
        let cookie = if self.session_cookie.is_some() {
            "(set)"
        } else {
            "(not set)"
        };
        format!(
            "runner         = {:?}\nproton_path    = {}\nsession_cookie = {}",
            self.runner, self.proton_path, cookie
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_config_is_owner_only_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("joltux-test-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = Paths::in_dir(&dir);
        // Simulate an older, world-readable config file.
        fs::create_dir_all(&dir).unwrap();
        fs::write(&paths.config_file, "{}").unwrap();

        let mut config = Config::default();
        config.set("session_cookie", "secret").unwrap();
        config.set("runner", "wine").unwrap();
        config.save(&paths).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&paths.config_file)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let loaded = Config::load(&paths).unwrap();
        assert_eq!(loaded.session_cookie.as_deref(), Some("secret"));
        assert_eq!(loaded.runner, Runner::Wine);
        let _ = fs::remove_dir_all(&dir);
    }
}
