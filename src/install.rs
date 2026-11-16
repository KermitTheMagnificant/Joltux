//! Downloading and unpacking a build.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;

use crate::api::USER_AGENT;

/// Streams `url` to `dest`, printing progress. Returns the bytes written.
///
/// A separate HTTP client is used on purpose: the download URL points at a
/// CDN, and we don't want to send the Game Jolt session cookie there. The
/// timeout is disabled because reqwest's blocking timeout covers the *whole*
/// request including the body, which would kill any large download.
pub fn download(url: &str, dest: &Path) -> Result<u64> {
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(None)
        .build()
        .context("failed to build download client")?;

    let mut resp = client
        .get(url)
        .send()
        .context("download request failed")?
        .error_for_status()
        .context("download server returned an error")?;

    let total = resp.content_length();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(dest).with_context(|| format!("cannot create {}", dest.display()))?;

    let mut buf = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut last_report: u64 = 0;
    loop {
        let n = resp.read(&mut buf).context("connection dropped mid-download")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        written += n as u64;
        // Update the progress line about once per MiB to keep output calm.
        if written - last_report >= 1024 * 1024 {
            last_report = written;
            print_progress(written, total);
        }
    }
    print_progress(written, total);
    eprintln!();
    Ok(written)
}

fn print_progress(done: u64, total: Option<u64>) {
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    match total {
        Some(t) if t > 0 => eprint!("\r  {:.1} / {:.1} MiB ({}%)   ", mib(done), mib(t), done * 100 / t),
        _ => eprint!("\r  {:.1} MiB   ", mib(done)),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    TarGz,
    Unknown,
}

/// Identify an archive by its first bytes rather than trusting file names.
fn sniff(path: &Path) -> Result<ArchiveKind> {
    let mut magic = [0u8; 4];
    let mut f = File::open(path)?;
    let n = f.read(&mut magic)?;
    Ok(match &magic[..n] {
        [0x50, 0x4b, 0x03, 0x04, ..] => ArchiveKind::Zip,
        [0x1f, 0x8b, ..] => ArchiveKind::TarGz,
        _ => ArchiveKind::Unknown,
    })
}

/// Unpacks `archive` into `dest`. Supports .zip and .tar.gz.
pub fn extract(archive: &Path, dest: &Path, archive_type_hint: &str) -> Result<()> {
    fs::create_dir_all(dest)?;
    match sniff(archive)? {
        ArchiveKind::Zip => extract_zip(archive, dest),
        ArchiveKind::TarGz => extract_tar_gz(archive, dest),
        ArchiveKind::Unknown => bail!(
            "unsupported archive format (site says `{archive_type_hint}`); only .zip and .tar.gz are supported so far"
        ),
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?).context("not a valid zip file")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;

        // `enclosed_name` is None for paths that escape the folder (`../x`,
        // absolute paths). Skipping them blocks "zip slip" attacks.
        let Some(rel) = entry.enclosed_name().map(|p| p.to_owned()) else {
            eprintln!("  skipping unsafe path in archive: {}", entry.name());
            continue;
        };
        let out = dest.join(rel);

        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&out).with_context(|| format!("cannot create {}", out.display()))?;
        io::copy(&mut entry, &mut f)?;

        // Keep the executable bit: native Linux builds need it.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(GzDecoder::new(File::open(archive)?));
    tar.set_preserve_permissions(true);
    for entry in tar.entries().context("not a valid tar.gz")? {
        let mut entry = entry?;
        // `unpack_in` refuses paths that would land outside `dest`.
        if !entry.unpack_in(dest)? {
            eprintln!("  skipping unsafe path in archive");
        }
    }
    Ok(())
}

/// Locates the file to launch inside an extracted build.
///
/// `wanted` comes from the launch option and is relative to the build root,
/// possibly with Windows `\` separators. If the exact path isn't there (some
/// archives wrap everything in one extra folder), we look for a file whose path
/// ends the same way, then for one with the same file name.
pub fn find_executable(root: &Path, wanted: &str) -> Option<PathBuf> {
    let wanted = wanted.replace('\\', "/");
    let wanted = wanted.trim_start_matches("./");

    let direct = root.join(wanted);
    if direct.is_file() {
        return Some(direct);
    }

    let wanted_lower = wanted.to_lowercase();
    let wanted_name = Path::new(wanted).file_name()?.to_string_lossy().to_lowercase();

    let mut files = Vec::new();
    walk(root, &mut files);

    let rel = |p: &PathBuf| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
    };
    files
        .iter()
        .find(|p| rel(p).ends_with(&wanted_lower))
        .or_else(|| {
            files
                .iter()
                .find(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()) == Some(wanted_name.clone()))
        })
        .cloned()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // file_type() doesn't follow symlinks, so a symlink loop can't trap us.
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("joltux-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_exact_and_wrapped_paths() {
        let root = scratch("find");
        fs::create_dir_all(root.join("MyGame-1.0/bin")).unwrap();
        fs::write(root.join("MyGame-1.0/bin/Game.exe"), b"x").unwrap();

        // Windows-style path, but the archive has an extra top folder.
        let found = find_executable(&root, "bin\\Game.exe").unwrap();
        assert!(found.ends_with("MyGame-1.0/bin/Game.exe"));

        // Case-insensitive file-name fallback.
        assert!(find_executable(&root, "game.EXE").is_some());
        assert!(find_executable(&root, "missing.exe").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn zip_roundtrip_keeps_exec_bit_and_blocks_traversal() {
        use zip::write::FileOptions;
        let root = scratch("zip");
        let zip_path = root.join("a.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = FileOptions::default().unix_permissions(0o755);
            w.start_file("run.sh", opts).unwrap();
            w.write_all(b"#!/bin/sh\n").unwrap();
            w.start_file("../evil.txt", opts).unwrap();
            w.write_all(b"nope").unwrap();
            w.finish().unwrap();
        }
        let out = root.join("out");
        extract(&zip_path, &out, "zip").unwrap();
        assert!(out.join("run.sh").is_file());
        assert!(!root.join("evil.txt").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(out.join("run.sh")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let _ = fs::remove_dir_all(&root);
    }
}
