//! Installing and configuring Windows-game runtimes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::api::USER_AGENT;
use crate::checks;
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

    /// Arguments that install packages without asking questions.
    fn install_arguments(self) -> &'static [&'static str] {
        match self {
            Self::Apt => &["install", "-y"],
            Self::Dnf => &["install", "-y"],
            Self::Pacman => &["-S", "--needed", "--noconfirm"],
            Self::Zypper => &["--non-interactive", "install"],
        }
    }

    /// 32-bit C library, Vulkan loader and Mesa drivers. Proton's container
    /// needs the 32-bit loader to set up graphics for 32-bit games, which
    /// most Windows games on Game Jolt are.
    fn lib32_packages(self) -> &'static [&'static str] {
        match self {
            Self::Apt => &[
                "libc6:i386",
                "libvulkan1:i386",
                "mesa-vulkan-drivers:i386",
                "libgl1:i386",
                "libgl1-mesa-dri:i386",
            ],
            Self::Dnf => &[
                "glibc.i686",
                "vulkan-loader.i686",
                "mesa-vulkan-drivers.i686",
                "mesa-dri-drivers.i686",
                "libglvnd-glx.i686",
            ],
            Self::Pacman => &[
                "lib32-glibc",
                "lib32-vulkan-icd-loader",
                "lib32-mesa",
                "lib32-vulkan-radeon",
                "lib32-vulkan-intel",
            ],
            Self::Zypper => &[
                "glibc-32bit",
                "libvulkan1-32bit",
                "Mesa-libGL1-32bit",
                "Mesa-dri-32bit",
                "libvulkan_radeon-32bit",
                "libvulkan_intel-32bit",
            ],
        }
    }
}

pub fn setup(paths: &Paths, config: &mut Config) -> Result<()> {
    let manager = detect_package_manager()?;
    install_system_packages(manager)?;

    println!("Installing the latest umu-launcher for this user...");
    let umu_path = install_umu(paths)?;

    config.runner = Runner::Umu;
    config.umu_path = umu_path.to_string_lossy().into_owned();
    config.wine_path = "wine".to_string();
    config.proton_path = "GE-Proton".to_string();
    config.save(paths)?;

    let problems = checks::find_problems(true);
    if !problems.is_empty() {
        eprintln!("Setup finished, but games may still fail to start:");
        for problem in problems {
            eprintln!("  - {problem}");
        }
    }
    Ok(())
}

/// Installs Wine plus the 32-bit graphics libraries that 32-bit Windows games
/// need. Everything goes in one package-manager run when possible, so the
/// user is asked for their password once. Only a Wine failure is fatal: 64-bit
/// and native games work without the 32-bit libraries, and the launcher
/// explains what is missing if a game later fails.
fn install_system_packages(manager: PackageManager) -> Result<()> {
    let (lib32, nvidia) = match prepare_lib32(manager) {
        Ok(()) => (
            manager.lib32_packages().iter().map(|p| p.to_string()).collect(),
            nvidia_lib32_package(manager),
        ),
        Err(error) => {
            eprintln!("Warning: skipping 32-bit graphics libraries: {error:#}");
            (Vec::new(), None)
        }
    };

    println!(
        "Installing Wine and 32-bit graphics libraries using {}...",
        manager.executable()
    );
    let mut all = vec!["wine".to_string()];
    all.extend(lib32.iter().cloned());
    all.extend(nvidia.clone());
    if install_packages(manager, &all).is_ok() {
        return Ok(());
    }

    // One unavailable package fails the whole run, so retry in steps to find
    // which part is the problem.
    eprintln!("Retrying the installation one part at a time...");
    install_packages(manager, &["wine"]).context("Wine installation failed")?;
    if lib32.is_empty() {
        return Ok(());
    }
    if let Err(error) = install_packages(manager, &lib32) {
        eprintln!("Warning: could not install 32-bit graphics libraries: {error:#}");
        return Ok(());
    }
    if let Some(nvidia) = nvidia {
        if install_packages(manager, &[&nvidia]).is_err() {
            eprintln!(
                "Warning: could not install the 32-bit NVIDIA driver package `{nvidia}`; install \
                 it with your package manager to run 32-bit games on your NVIDIA GPU"
            );
        }
    }
    Ok(())
}

/// Makes 32-bit packages installable, where the distro needs that first.
fn prepare_lib32(manager: PackageManager) -> Result<()> {
    match manager {
        PackageManager::Apt => run_checked(
            // One privileged command so the user is only asked once.
            privileged("sh")?.args(["-c", "dpkg --add-architecture i386 && apt-get update"]),
            "could not enable 32-bit (i386) packages",
        ),
        PackageManager::Pacman => {
            let conf = fs::read_to_string("/etc/pacman.conf").unwrap_or_default();
            if !conf.lines().any(|line| line.trim() == "[multilib]") {
                bail!(
                    "they need the multilib repository; enable [multilib] in /etc/pacman.conf \
                     and run `joltux setup` again"
                );
            }
            Ok(())
        }
        PackageManager::Dnf | PackageManager::Zypper => Ok(()),
    }
}

/// The 32-bit counterpart of the installed proprietary NVIDIA GL package, if
/// any. It has to come from the same repository and match its version.
fn nvidia_lib32_package(manager: PackageManager) -> Option<String> {
    match manager {
        PackageManager::Dnf => ["xorg-x11-drv-nvidia-libs", "nvidia-driver-libs"]
            .into_iter()
            .find(|name| query_succeeds("rpm", &["-q", name]))
            .map(|name| format!("{name}.i686")),
        PackageManager::Zypper => ["nvidia-gl-G06", "nvidia-gl-G05"]
            .into_iter()
            .find(|name| query_succeeds("rpm", &["-q", name]))
            .map(|name| format!("{name}-32bit")),
        PackageManager::Pacman => query_succeeds("pacman", &["-Q", "nvidia-utils"])
            .then(|| "lib32-nvidia-utils".to_string()),
        PackageManager::Apt => {
            // Ubuntu and Debian name it after the driver branch: libnvidia-gl-550.
            let out = Command::new("dpkg-query")
                .args(["-W", "-f", "${Package} ${db:Status-Status}\n", "libnvidia-gl-*"])
                .stderr(Stdio::null())
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
                let (name, status) = line.split_once(' ')?;
                (status == "installed").then(|| format!("{name}:i386"))
            })
        }
    }
}

fn query_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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

fn install_packages<S: AsRef<std::ffi::OsStr>>(manager: PackageManager, packages: &[S]) -> Result<()> {
    let mut command = privileged(manager.executable())?;
    command.args(manager.install_arguments()).args(packages);
    run_checked(&mut command, "the package manager could not install the packages")
}

/// Set by the GUI on the commands it runs. There is no terminal for sudo to
/// ask for a password, so root commands go through pkexec, which shows the
/// desktop's own password dialog.
pub const GRAPHICAL_AUTH_ENV: &str = "JOLTUX_GRAPHICAL_AUTH";

/// A command that runs as root: directly if Joltux already is root, through
/// pkexec when run from the GUI, otherwise through sudo.
fn privileged(program: &str) -> Result<Command> {
    if is_root() {
        return Ok(Command::new(program));
    }
    let graphical = std::env::var_os(GRAPHICAL_AUTH_ENV).is_some();
    let helper = if graphical {
        if !command_exists("pkexec") {
            bail!("pkexec (polkit) is required to install system packages from the app; run `joltux setup` in a terminal instead");
        }
        "pkexec"
    } else {
        if !command_exists("sudo") {
            bail!("sudo is required to install system packages; run the package-manager command manually");
        }
        "sudo"
    };
    let mut command = Command::new(helper);
    command.arg(program);
    Ok(command)
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
