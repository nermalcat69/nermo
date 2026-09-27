use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
        let url = format!("{}/{}/{}", self.registry, encode_name(name), version);
        let resp = self.http.get(&url).send().with_context(|| format!("requesting {url}"))?;
        if !resp.status().is_success() {
            bail!("registry returned {} for {name}@{version}", resp.status());
        }
        resp.json().with_context(|| format!("parsing metadata for {name}@{version}"))
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
    }

    /// Download a package tarball, following redirects (handled by the HTTP client).
    pub fn download_tarball(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().with_context(|| format!("downloading {url}"))?;
        if !resp.status().is_success() {
            bail!("download failed with status {} for {url}", resp.status());
        }
        Ok(resp.bytes().with_context(|| format!("reading response body from {url}"))?.to_vec())
    }
}

/// Scoped packages (@scope/name) must have the slash percent-encoded in the URL path.
fn encode_name(name: &str) -> String {
    name.replace('/', "%2f")
}
