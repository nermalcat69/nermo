use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// Retry a network operation a bounded number of times with a short backoff.
/// A resolve over hundreds of packages makes hundreds of HTTP requests; a
/// single transient failure (a dropped HTTP/2 stream, a mid-transfer reset)
/// shouldn't abort the whole thing when trying again a moment later usually
/// just works. Not applied to `check_connectivity`, which is meant to fail
/// fast for diagnostics, not paper over a real outage.
fn with_retries<T>(mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = None;
    for i in 0..MAX_ATTEMPTS {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(e) => {
                if i + 1 < MAX_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(300 * u64::from(i + 1)));
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("loop runs at least once"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionMetadata {
    pub name: String,
    pub version: String,
    pub dist: Dist,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// Dependencies that are only needed on some platforms (npm convention:
    /// per-platform native binaries, e.g. esbuild/swc/rolldown binding
    /// packages), or that installers may skip entirely on failure.
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: BTreeMap<String, String>,
    /// If present, this package is only valid on the listed OS/CPU values
    /// (npm's own `os`/`cpu` package.json fields), possibly negated with a
    /// leading '!'. `None` means "all platforms".
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub cpu: Option<Vec<String>>,
    /// npm's package.json `bin` field, either a single path (binary name
    /// defaults to the package's own unscoped name) or a name->path map.
    #[serde(default, rename = "bin")]
    pub bin_field: Option<BinField>,
    /// npm's `peerDependencies`: not fetched/resolved independently (see
    /// `Resolver::link_peer_dependencies`) — only linked to an already-
    /// resolved compatible version elsewhere in the graph, same as pnpm.
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BinField {
    Single(String),
    Named(BTreeMap<String, String>),
}

impl VersionMetadata {
    /// Resolve `bin` into its final name -> script-path form, applying
    /// npm's own-name convention for the single-path shorthand (the binary
    /// name is the package's name with any `@scope/` stripped).
    pub fn bin_entries(&self) -> BTreeMap<String, String> {
        match &self.bin_field {
            None => BTreeMap::new(),
            Some(BinField::Named(map)) => map.clone(),
            Some(BinField::Single(path)) => {
                let name = self.name.rsplit('/').next().unwrap_or(&self.name);
                BTreeMap::from([(name.to_string(), path.clone())])
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dist {
    pub tarball: String,
    /// "<algo>-<base64>", e.g. "sha512-abcd...". Older packages may lack this
    /// and only provide `shasum` (hex-encoded SHA-1).
    pub integrity: Option<String>,
    pub shasum: Option<String>,
}

/// The full package document: every published version and its metadata.
/// Needed by the resolver, which must see all versions to pick the best one
/// satisfying a range; the single-version endpoint isn't enough for that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub versions: BTreeMap<String, VersionMetadata>,
}

pub struct Client {
    http: reqwest::blocking::Client,
    registry: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("nermo/", env!("CARGO_PKG_VERSION")))
            // A long-lived, widely-depended-on package (e.g. `wrangler`) can
            // have hundreds of published versions each listing dozens of
            // dependencies; even npm's abbreviated metadata format for that
            // is tens of MB. Tarballs for native-heavy packages can be large
            // too. 30s was tuned for a fast connection and a small package;
            // a real timeout should only fire when nothing is happening at
            // all, not when a large-but-progressing transfer is slow.
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .context("building HTTP client")?;
        Ok(Self { http, registry: DEFAULT_REGISTRY.to_string() })
    }

    /// A quick, short-timeout reachability check for `nermo doctor` — not
    /// used on the normal install path, where a real request failing with a
    /// real error is more informative than a separate up-front probe.
    pub fn check_connectivity(&self) -> Result<()> {
        let resp = self
            .http
            .get(&self.registry)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .with_context(|| format!("connecting to {}", self.registry))?;
        if resp.status().is_success() || resp.status().is_redirection() {
            Ok(())
        } else {
            bail!("registry responded with {}", resp.status());
        }
    }

    /// Fetch metadata for one exact package version.
    /// Uses the registry's abbreviated per-version endpoint rather than the
    /// full package document, since we don't need every published version.
    pub fn version_metadata(&self, name: &str, version: &str) -> Result<VersionMetadata> {
        with_retries(|| {
            let url = format!("{}/{}/{}", self.registry, encode_name(name), version);
            let resp = self.http.get(&url).send().with_context(|| format!("requesting {url}"))?;
            if !resp.status().is_success() {
                bail!("registry returned {} for {name}@{version}", resp.status());
            }
            resp.json().with_context(|| format!("parsing metadata for {name}@{version}"))
        })
    }

    /// Fetch the package document listing every published version.
    ///
    /// Requests npm's "abbreviated" metadata format (the same one npm/pnpm
    /// use for installs) instead of the default full document. For a
    /// long-lived, popular package like `@types/node` the full document is
    /// over 11MB (every version's readme, full dependency history, etc.);
    /// the abbreviated one is ~2MB and still has everything the resolver
    /// needs (name, version, dependencies, optionalDependencies, os, cpu,
    /// dist). On a slow connection the full document was enough to time out
    /// requests outright — this is a real fix for that, not just a bigger
    /// timeout number.
    pub fn package_metadata(&self, name: &str) -> Result<PackageMetadata> {
        with_retries(|| {
            let url = format!("{}/{}", self.registry, encode_name(name));
            let resp = self
                .http
                .get(&url)
                .header(reqwest::header::ACCEPT, "application/vnd.npm.install-v1+json")
                .send()
                .with_context(|| format!("requesting {url}"))?;
            if !resp.status().is_success() {
                bail!("registry returned {} for {name}", resp.status());
            }
            resp.json().with_context(|| format!("parsing package metadata for {name}"))
        })
    }

    /// Download a package tarball, following redirects (handled by the HTTP client).
    pub fn download_tarball(&self, url: &str) -> Result<Vec<u8>> {
        with_retries(|| {
            let resp = self.http.get(url).send().with_context(|| format!("downloading {url}"))?;
            if !resp.status().is_success() {
                bail!("download failed with status {} for {url}", resp.status());
            }
            Ok(resp.bytes().with_context(|| format!("reading response body from {url}"))?.to_vec())
        })
    }
}

/// Scoped packages (@scope/name) must have the slash percent-encoded in the URL path.
fn encode_name(name: &str) -> String {
    name.replace('/', "%2f")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn meta(name: &str, bin_field: Option<BinField>) -> VersionMetadata {
        VersionMetadata {
            name: name.into(),
            version: "1.0.0".into(),
            dist: Dist { tarball: "https://example/x.tgz".into(), integrity: None, shasum: None },
            dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            os: None,
            cpu: None,
            bin_field,
            peer_dependencies: BTreeMap::new(),
        }
    }

    #[test]
    fn bin_entries_is_empty_when_no_bin_field() {
        assert!(meta("demo", None).bin_entries().is_empty());
    }

    #[test]
    fn bin_entries_named_map_passes_through_unchanged() {
        let map = BTreeMap::from([("foo".to_string(), "./bin/foo.js".to_string())]);
        assert_eq!(meta("demo", Some(BinField::Named(map.clone()))).bin_entries(), map);
    }

    #[test]
    fn bin_entries_single_string_uses_unscoped_package_name() {
        let entries = meta("@scope/cli-tool", Some(BinField::Single("./bin/run.js".into()))).bin_entries();
        assert_eq!(entries, BTreeMap::from([("cli-tool".to_string(), "./bin/run.js".to_string())]));
    }

    #[test]
    fn bin_entries_single_string_unscoped_name_is_used_as_is() {
        let entries = meta("plain-cli", Some(BinField::Single("./bin/run.js".into()))).bin_entries();
        assert_eq!(entries, BTreeMap::from([("plain-cli".to_string(), "./bin/run.js".to_string())]));
    }

    #[test]
    fn with_retries_succeeds_after_transient_failures() {
        let attempts = AtomicU32::new(0);
        let result = with_retries(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                bail!("transient failure {n}");
            }
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn with_retries_gives_up_after_max_attempts() {
        let attempts = AtomicU32::new(0);
        let result: Result<()> = with_retries(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            bail!("always fails")
        });
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3, "should stop after the max, not retry forever");
    }
}
