//! User settings, stored as JSON in the config directory.

use std::fs;

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
        fs::write(&paths.config_file, serde_json::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&paths.config_file, fs::Permissions::from_mode(0o600))?;
        }
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
                self.session_cookie = if value.is_empty() { None } else { Some(value.to_string()) }
            }
            other => bail!("unknown setting `{other}` (try: runner, proton_path, session_cookie)"),
        }
        Ok(())
    }

    /// Settings for display, with the cookie hidden.
    pub fn redacted(&self) -> String {
        let cookie = if self.session_cookie.is_some() { "(set)" } else { "(not set)" };
        format!(
            "runner         = {:?}\nproton_path    = {}\nsession_cookie = {}",
            self.runner, self.proton_path, cookie
        )
    }
}
