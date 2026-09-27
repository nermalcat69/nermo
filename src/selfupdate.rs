use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO: &str = "nermalcat69/nermo";
/// How often the background check (`notify_if_update_available`) is allowed
/// to actually hit the network — every other invocation reads the cached
/// result instead, so `nermo` doesn't add a GitHub API round trip to every
/// single command.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Serialize, Deserialize)]
struct CachedCheck {
    checked_at_unix: u64,
    latest_tag: String,
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The exact target-triple suffix `.github/workflows/release.yml` builds
/// for. Nothing published for a platform not in this list.
fn target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn is_newer(current: &str, latest_tag: &str) -> bool {
    let latest = latest_tag.trim_start_matches('v');
    match (semver::Version::parse(latest), semver::Version::parse(current)) {
        (Ok(l), Ok(c)) => l > c,
        // Can't compare (malformed tag, dev build, ...): don't claim an
        // update is available on a parse hiccup.
        _ => false,
    }
}

fn fetch_latest_release(timeout: Duration) -> Result<Release> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("nermo/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .build()
        .context("building HTTP client")?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = client.get(&url).send().with_context(|| format!("requesting {url}"))?;
    if !resp.status().is_success() {
        bail!("GitHub API returned {}", resp.status());
    }
    resp.json().context("parsing GitHub release response")
}

/// A cheap, cached, best-effort update check meant to run on every ordinary
/// command. Every failure mode (offline, GitHub API down, cache unreadable)
/// is swallowed silently — this must never fail or slow down the command
/// it's piggybacking on. Prints to stderr, not stdout, so it never pollutes
/// piped/scripted output.
pub fn notify_if_update_available() {
    let Ok(store) = crate::store::Store::open() else { return };
    let cache_path = store.root().join("metadata").join("update-check.json");

    if let Ok(text) = std::fs::read_to_string(&cache_path) {
        if let Ok(cached) = serde_json::from_str::<CachedCheck>(&text) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            if now.saturating_sub(cached.checked_at_unix) < CHECK_INTERVAL.as_secs() {
                if is_newer(current_version(), &cached.latest_tag) {
                    print_notice(&cached.latest_tag);
                }
                return;
            }
        }
    }

    let Ok(release) = fetch_latest_release(Duration::from_secs(3)) else { return };
    let _ = std::fs::create_dir_all(store.root().join("metadata"));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let cached = CachedCheck { checked_at_unix: now, latest_tag: release.tag_name.clone() };
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(&cache_path, json);
    }
    if is_newer(current_version(), &release.tag_name) {
        print_notice(&release.tag_name);
    }
}

fn print_notice(latest_tag: &str) {
    eprintln!("\nA new version of nermo is available: {} -> {latest_tag}. Run `nermo upgrade` to update.", current_version());
}

/// Download and install the latest release in place of the currently
/// running binary.
pub fn upgrade() -> Result<()> {
    println!("Current version: {}", current_version());
    println!("Checking for updates...");
    let release = fetch_latest_release(Duration::from_secs(10)).context("checking the latest release")?;

    if !is_newer(current_version(), &release.tag_name) {
        println!("Already up to date ({}).", current_version());
        return Ok(());
    }

    let target = target_triple().ok_or_else(|| {
        anyhow!("no prebuilt nermo binary is published for this platform ({} {})", std::env::consts::OS, std::env::consts::ARCH)
    })?;
    let is_windows = target.contains("windows");
    let ext = if is_windows { "zip" } else { "tar.gz" };
    let asset_name = format!("nermo-{}-{target}.{ext}", release.tag_name);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("release {} has no asset named {asset_name}", release.tag_name))?;

    println!("Downloading {} ({})...", release.tag_name, asset.name);
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("nermo/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(180))
        .build()
        .context("building HTTP client")?;
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .with_context(|| format!("downloading {}", asset.browser_download_url))?
        .bytes()
        .context("reading downloaded archive")?;

    let binary_name = if is_windows { "nermo.exe" } else { "nermo" };
    let binary_bytes = if is_windows { extract_from_zip(&bytes, binary_name)? } else { extract_from_tar_gz(&bytes, binary_name)? };

    let current_exe = std::env::current_exe().context("locating the running nermo executable")?;
    replace_executable(&current_exe, &binary_bytes)?;

    println!("Updated to {}.", release.tag_name);
    Ok(())
}

fn extract_from_tar_gz(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("reading downloaded archive")? {
        let mut entry = entry.context("reading archive entry")?;
        let path = entry.path().context("reading entry path")?.into_owned();
        if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).context("reading binary from archive")?;
            return Ok(buf);
        }
    }
    bail!("{binary_name} was not found in the downloaded archive")
}

fn extract_from_zip(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("reading downloaded zip archive")?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("reading zip entry")?;
        let matches = Path::new(file.name()).file_name().and_then(|n| n.to_str()) == Some(binary_name);
        if matches {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).context("reading binary from zip")?;
            return Ok(buf);
        }
    }
    bail!("{binary_name} was not found in the downloaded archive")
}

/// Atomically replace `current` (the running executable) with `new_bytes`.
#[cfg(unix)]
fn replace_executable(current: &Path, new_bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = current.parent().context("executable has no parent directory")?;
    let tmp = dir.join(".nermo-upgrade.tmp");

    match std::fs::write(&tmp, new_bytes) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            bail!("permission denied writing to {}; try `sudo nermo upgrade`", dir.display())
        }
        Err(e) => return Err(e).with_context(|| format!("writing {}", tmp.display())),
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("marking {} executable", tmp.display()))?;
    // rename() over a running executable is safe on Unix: the OS keeps the
    // old inode's data alive for this process's own already-open handle to
    // it, while the directory entry immediately points at the new file for
    // the next invocation.
    std::fs::rename(&tmp, current).with_context(|| format!("replacing {}", current.display()))?;
    Ok(())
}

// ponytail: Windows generally won't let a running process's own .exe be
// renamed or overwritten in place. Rather than ship a self-replace path
// that's never been run on real Windows, stage the new binary alongside the
// old one and tell the user to finish the swap themselves after exiting.
#[cfg(windows)]
fn replace_executable(current: &Path, new_bytes: &[u8]) -> Result<()> {
    let staged = current.with_file_name("nermo.new.exe");
    std::fs::write(&staged, new_bytes).with_context(|| format!("writing {}", staged.display()))?;
    println!(
        "Downloaded the new version to:\n  {}\nClose this terminal, then replace the old binary with it:\n  move /Y \"{}\" \"{}\"",
        staged.display(),
        staged.display(),
        current.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_compares_semver_ignoring_v_prefix() {
        assert!(is_newer("0.1.2", "v0.1.3"));
        assert!(!is_newer("0.1.2", "v0.1.2"));
        assert!(!is_newer("0.1.2", "v0.1.1"));
    }

    #[test]
    fn is_newer_is_false_on_unparseable_input() {
        assert!(!is_newer("0.1.2", "not-a-version"));
        assert!(!is_newer("not-a-version", "v0.1.2"));
    }

    #[test]
    fn target_triple_matches_a_release_asset_or_is_none() {
        // Whatever this evaluates to on the test-running machine, it must be
        // one of the exact triples release.yml actually builds, so `upgrade`
        // constructs an asset name that can really exist.
        if let Some(target) = target_triple() {
            assert!(
                ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
                    .contains(&target)
            );
        }
    }

    #[test]
    fn extract_from_tar_gz_finds_the_binary_inside_a_wrapper_directory() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        let contents = b"fake binary contents";
        header.set_size(contents.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, "nermo-v0.1.3-x86_64-unknown-linux-gnu/nermo", &contents[..]).unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let extracted = extract_from_tar_gz(&gz_bytes, "nermo").unwrap();
        assert_eq!(extracted, contents);
    }

    #[test]
    fn extract_from_zip_finds_the_binary_inside_a_wrapper_directory() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer.start_file("nermo-v0.1.3-x86_64-pc-windows-msvc/nermo.exe", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut writer, b"fake exe contents").unwrap();
            writer.finish().unwrap();
        }

        let extracted = extract_from_zip(buf.get_ref(), "nermo.exe").unwrap();
        assert_eq!(extracted, b"fake exe contents");
    }

    #[test]
    fn extract_missing_binary_is_an_error() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_cksum();
        builder.append_data(&mut header, "wrapper/README.md", &[][..]).unwrap();
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        assert!(extract_from_tar_gz(&gz_bytes, "nermo").is_err());
    }
}
