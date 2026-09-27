use crate::registry::Dist;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest as _, Sha512};
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

/// Verify tarball bytes against the registry-supplied integrity hash.
/// Prefers the modern `integrity` field (Subresource Integrity format,
/// "sha512-<base64>"); falls back to the legacy hex SHA-1 `shasum`.
pub fn verify(bytes: &[u8], dist: &Dist) -> Result<()> {
    if let Some(integrity) = &dist.integrity {
        let (algo, expected_b64) = integrity
            .split_once('-')
            .ok_or_else(|| anyhow!("malformed integrity string: {integrity}"))?;
        if algo != "sha512" {
            bail!("unsupported integrity algorithm: {algo}");
        }
        let expected = STANDARD.decode(expected_b64).context("decoding integrity base64")?;
        let actual = Sha512::digest(bytes);
        if actual.as_slice() != expected.as_slice() {
            bail!("integrity check failed: expected sha512-{expected_b64}, got mismatched content");
        }
        return Ok(());
    }

    if let Some(shasum) = &dist.shasum {
        let actual = hex::encode(sha1::Sha1::digest(bytes));
        if !actual.eq_ignore_ascii_case(shasum) {
            bail!("shasum check failed: expected {shasum}, got {actual}");
        }
        return Ok(());
    }

    bail!("registry provided no integrity or shasum to verify against");
}

/// Extract a gzipped tarball into `dest`, which must already exist.
/// npm tarballs wrap contents in a single top-level `package/` directory;
/// that prefix is stripped so `dest` becomes the package root.
pub fn extract(bytes: &[u8], dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries().context("reading tar entries")? {
        let mut entry = entry.context("reading tar entry")?;
        let raw_path = entry.path().context("reading entry path")?.into_owned();

        // Drop the leading "package/" component npm always includes.
        let relative: PathBuf = raw_path.components().skip(1).collect();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = safe_join(dest, &relative)
            .ok_or_else(|| anyhow!("archive entry escaped extraction directory: {}", relative.display()))?;

        // `Entry::unpack` recreates a symlink entry using its stored link
        // target verbatim, with no check that the target stays inside
        // `dest` (PRD §16.2). npm strips symlinks from published packages,
        // so a symlink entry here is unexpected; refuse it rather than
        // trying to validate an escape-prone target.
        if entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link() {
            bail!("archive entry {} is a symlink, which nermo refuses to extract", relative.display());
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        entry.unpack(&target).with_context(|| format!("extracting {}", relative.display()))?;
    }
    Ok(())
}

/// Join `base` with `relative`, rejecting any path that would escape `base`
/// via `..` components, an embedded root, or a prefix (Windows drive letter).
fn safe_join(base: &Path, relative: &Path) -> Option<PathBuf> {
    let mut result = base.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip_tar(entries: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        entries(&mut builder);
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extracts_regular_files_under_the_package_prefix() {
        let bytes = gzip_tar(|b| {
            let mut header = tar::Header::new_gnu();
            let contents = b"module.exports = 1;";
            header.set_size(contents.len() as u64);
            header.set_cksum();
            b.append_data(&mut header, "package/index.js", &contents[..]).unwrap();
        });

        let dest = tempfile::tempdir().unwrap();
        extract(&bytes, dest.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dest.path().join("index.js")).unwrap(), "module.exports = 1;");
    }

    #[test]
    fn refuses_to_extract_a_symlink_entry() {
        let bytes = gzip_tar(|b| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_link_name("../../../etc/passwd").unwrap();
            header.set_cksum();
            b.append_data(&mut header, "package/evil-link", &[][..]).unwrap();
        });

        let dest = tempfile::tempdir().unwrap();
        let err = extract(&bytes, dest.path()).unwrap_err();
        assert!(err.to_string().contains("symlink"));
    }

    #[test]
    fn rejects_path_traversal_entries() {
        // `tar::Builder::append_data` refuses to build a path containing
        // "..", which is exactly why this test can't use it: a hand-crafted
        // adversarial tarball isn't bound by this crate's own guardrails, so
        // the name is written directly into the raw header to bypass them
        // and actually exercise `extract`'s own defense (`safe_join`).
        let bytes = gzip_tar(|b| {
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            let name = b"package/../../escape.txt";
            header.as_mut_bytes()[..name.len()].copy_from_slice(name);
            header.set_cksum();
            b.append(&header, &b"evil"[..]).unwrap();
        });

        let dest = tempfile::tempdir().unwrap();
        let err = extract(&bytes, dest.path()).unwrap_err();
        assert!(err.to_string().contains("escaped extraction directory"));
    }
}
