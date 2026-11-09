//! Installing and configuring Windows-game runtimes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::api::USER_AGENT;
use crate::config::{Config, Runner};
use crate::paths::Paths;

const UMU_RELEASE_URL: &str =
    "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest";
const UMU_GIT_URL: &str = "https://github.com/Open-Wine-Components/umu-launcher.git";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
}

impl PackageManager {
    fn executable(self) -> &'static str {
        match self {
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
        }
    }

    fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::Apt => &["install", "-y", "wine"],
            Self::Dnf => &["install", "-y", "wine"],
            Self::Pacman => &["-S", "--needed", "--noconfirm", "wine"],
            Self::Zypper => &["--non-interactive", "install", "wine"],
        }
    }
}

pub fn setup(paths: &Paths, config: &mut Config) -> Result<()> {
    let manager = detect_package_manager()?;
    println!("Installing Wine using {}...", manager.executable());
    install_system_wine(manager)?;

    println!("Installing the latest umu-launcher for this user...");
    let umu_path = install_umu(paths)?;

    config.runner = Runner::Umu;
    config.umu_path = umu_path.to_string_lossy().into_owned();
    config.wine_path = "wine".to_string();
    config.proton_path = "GE-Proton".to_string();
    config.save(paths)?;
    Ok(())
}

fn detect_package_manager() -> Result<PackageManager> {
    let release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let manager = package_manager_for(&release).context(
        "unsupported Linux distribution; install Wine with its package manager and try again",
    )?;
    if !command_exists(manager.executable()) {
        bail!(
            "{} was not found on PATH; install Wine manually using your distribution's package manager",
            manager.executable()
        );
    }
    Ok(manager)
}

fn package_manager_for(os_release: &str) -> Option<PackageManager> {
    let ids = os_release
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            matches!(key, "ID" | "ID_LIKE").then_some(value.trim_matches('"'))
        })
        .flat_map(str::split_ascii_whitespace)
        .collect::<Vec<_>>();

    if ids.iter().any(|id| {
        matches!(
            *id,
            "debian" | "ubuntu" | "linuxmint" | "pop" | "elementary" | "zorin"
        )
    }) {
        Some(PackageManager::Apt)
    } else if ids
        .iter()
        .any(|id| matches!(*id, "fedora" | "rhel" | "centos" | "rocky" | "almalinux"))
    {
        Some(PackageManager::Dnf)
    } else if ids
        .iter()
        .any(|id| matches!(*id, "arch" | "manjaro" | "endeavouros" | "garuda"))
    {
        Some(PackageManager::Pacman)
    } else if ids
        .iter()
        .any(|id| matches!(*id, "opensuse" | "opensuse-leap" | "opensuse-tumbleweed"))
    {
        Some(PackageManager::Zypper)
    } else {
        None
    }
}

fn install_system_wine(manager: PackageManager) -> Result<()> {
    let mut command = if is_root() {
        Command::new(manager.executable())
    } else {
        if !command_exists("sudo") {
            bail!("sudo is required to install Wine; run the package-manager command manually");
        }
        let mut command = Command::new("sudo");
        command.arg(manager.executable());
        command
    };
    command.args(manager.arguments());
    let status = command
        .status()
        .context("could not start the system package manager")?;
    if !status.success() {
        bail!("Wine installation failed (package manager exited with {status})");
    }
    Ok(())
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && output.stdout == b"0\n")
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

fn install_umu(paths: &Paths) -> Result<PathBuf> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("could not create GitHub client")?;
    let release: LatestRelease = client
        .get(UMU_RELEASE_URL)
        .send()
        .context("could not query the latest umu-launcher release")?
        .error_for_status()
        .context("GitHub returned an error for the umu-launcher release")?
        .json()
        .context("GitHub returned invalid release metadata")?;
    if !valid_tag(&release.tag_name) {
        bail!("GitHub returned an invalid umu-launcher release tag");
    }

    let python = Path::new("python3");
    if !command_exists("python3") {
        bail!("Python 3 is required to install umu-launcher into a user-local environment");
    }
    if !command_exists("git") {
        bail!("Git is required to install umu-launcher from its official release tag");
    }
    fs::create_dir_all(&paths.runtimes_dir)?;
    let environment = paths.runtimes_dir.join("umu");
    run_checked(
        Command::new(python).args(["-m", "venv"]).arg(&environment),
        "could not create the umu-launcher Python environment; install Python venv support",
    )?;

    let python = environment.join("bin/python");
    let release_source = format!("git+{UMU_GIT_URL}@{}", release.tag_name);
    run_checked(
        Command::new(&python)
            .args(["-m", "pip", "install", "--upgrade"])
            .arg(release_source),
        "could not install umu-launcher from its official GitHub release tag",
    )?;

    let executable = environment.join("bin/umu-run");
    if !executable.is_file() {
        bail!("umu-launcher installed without producing umu-run");
    }
    Ok(executable)
}

fn run_checked(command: &mut Command, message: &str) -> Result<()> {
    let status = command.status().context(message.to_string())?;
    if !status.success() {
        bail!("{message} (process exited with {status})");
    }
    Ok(())
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 128
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::{package_manager_for, valid_tag, PackageManager};

    #[test]
    fn detects_supported_distribution_families() {
        assert_eq!(
            package_manager_for("ID=ubuntu\nID_LIKE=debian"),
            Some(PackageManager::Apt)
        );
        assert_eq!(
            package_manager_for("ID=fedora\n"),
            Some(PackageManager::Dnf)
        );
        assert_eq!(
            package_manager_for("ID=manjaro\nID_LIKE=arch"),
            Some(PackageManager::Pacman)
        );
        assert_eq!(
            package_manager_for("ID=opensuse-tumbleweed\n"),
            Some(PackageManager::Zypper)
        );
        assert_eq!(package_manager_for("ID=org.freedesktop.platform\n"), None);
    }

    #[test]
    fn only_accepts_safe_release_tags() {
        assert!(valid_tag("1.4.4"));
        assert!(valid_tag("release-1_4"));
        assert!(!valid_tag("../main"));
        assert!(!valid_tag(""));
    }
}
