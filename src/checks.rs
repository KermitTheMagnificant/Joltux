//! Looks for common system problems that stop games from starting, so setup
//! and the launcher can explain them instead of showing only an exit code.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Where the 32-bit dynamic loader lives on the supported distributions.
/// Proton's container needs it to run its 32-bit helper programs, and 32-bit
/// games need it to load graphics drivers.
const LOADER_32: &[&str] = &[
    "/lib/ld-linux.so.2",
    "/usr/lib/ld-linux.so.2",
    "/usr/lib32/ld-linux.so.2",
];

/// Library folders, 64-bit ones first. Some of these hold 32-bit libraries on
/// one distro and 64-bit ones on another (/usr/lib), so callers check the
/// ELF class of what they find instead of trusting the folder name.
const LIB_DIRS: &[&str] = &[
    "/usr/lib64",
    "/usr/lib/x86_64-linux-gnu",
    "/usr/lib",
    "/usr/lib32",
    "/usr/lib/i386-linux-gnu",
];

/// A problem found on this system, with what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub what: String,
    pub fix: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n    Fix: {}", self.what, self.fix)
    }
}

/// Every problem found. `windows` adds checks that only matter for games run
/// through Proton or Wine (most Windows games on Game Jolt are 32-bit).
pub fn find_problems(windows: bool) -> Vec<Problem> {
    let mut problems = Vec::new();

    let nvidia_kernel = fs::read_to_string("/proc/driver/nvidia/version")
        .ok()
        .and_then(|text| kernel_driver_version(&text));
    let nvidia_libs = userspace_nvidia_version();
    if let (Some(kernel), Some(libs)) = (&nvidia_kernel, &nvidia_libs) {
        if kernel != libs {
            problems.push(Problem {
                what: format!(
                    "The running NVIDIA kernel driver ({kernel}) does not match the installed \
                     NVIDIA libraries ({libs}), so OpenGL and Vulkan cannot start."
                ),
                fix: "reboot so the updated driver loads.".into(),
            });
        }
    }

    if windows {
        if !LOADER_32.iter().any(|path| Path::new(path).exists()) {
            problems.push(Problem {
                what: "32-bit system libraries are not installed, so 32-bit games cannot reach \
                       the graphics driver."
                    .into(),
                fix: "run `joltux setup` (or the setup button in the app).".into(),
            });
        } else {
            if find_lib32("libvulkan.so.1").is_none() {
                problems.push(Problem {
                    what: "The 32-bit Vulkan loader is not installed, so 32-bit games cannot \
                           use the GPU."
                        .into(),
                    fix: "run `joltux setup` (or the setup button in the app).".into(),
                });
            }
            if nvidia_kernel.is_some() && find_lib32("libGLX_nvidia.so.0").is_none() {
                problems.push(Problem {
                    what: "The 32-bit NVIDIA driver libraries are not installed, so 32-bit \
                           games cannot use your NVIDIA GPU."
                        .into(),
                    fix: "run `joltux setup` (or the setup button in the app).".into(),
                });
            }
        }
    }

    problems
}

/// Pulls "620.70.06" out of /proc/driver/nvidia/version, whose first line
/// looks like "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  620.70.06  Release Build ...".
fn kernel_driver_version(text: &str) -> Option<String> {
    text.lines()
        .next()?
        .split_whitespace()
        .find(|word| looks_like_version(word))
        .map(str::to_owned)
}

/// The version of the installed 64-bit NVIDIA GL library, taken from its
/// file name (libGLX_nvidia.so.620.70.06).
fn userspace_nvidia_version() -> Option<String> {
    LIB_DIRS.iter().find_map(|dir| {
        fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix("libGLX_nvidia.so.")?;
            (looks_like_version(version) && elf_class(&entry.path()) == Some(64))
                .then(|| version.to_owned())
        })
    })
}

fn find_lib32(name: &str) -> Option<PathBuf> {
    LIB_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| elf_class(path) == Some(32))
}

/// 32 or 64 for an ELF file (following symlinks), None for anything else.
fn elf_class(path: &Path) -> Option<u8> {
    let mut header = [0u8; 5];
    File::open(path).ok()?.read_exact(&mut header).ok()?;
    match header {
        [0x7f, b'E', b'L', b'F', 1] => Some(32),
        [0x7f, b'E', b'L', b'F', 2] => Some(64),
        _ => None,
    }
}

/// "620.70.06" or "550.54" style: digits separated by at least one dot.
fn looks_like_version(word: &str) -> bool {
    word.contains('.')
        && word
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::{kernel_driver_version, looks_like_version};

    #[test]
    fn reads_kernel_driver_versions() {
        assert_eq!(
            kernel_driver_version(
                "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release Build  (dvs-builder@U22)  Tue Oct  6 04:56:08 UTC 2026\nGCC version: gcc 15"
            ),
            Some("615.78.08".into())
        );
        assert_eq!(
            kernel_driver_version("NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.54.14  Thu Feb 22 01:44:30 UTC 2024"),
            Some("550.54.14".into())
        );
        assert_eq!(kernel_driver_version(""), None);
    }

    #[test]
    fn recognises_versions() {
        assert!(looks_like_version("620.70.06"));
        assert!(looks_like_version("550.54"));
        assert!(!looks_like_version("x86_64"));
        assert!(!looks_like_version("0"));
        assert!(!looks_like_version("1..2"));
    }
}
