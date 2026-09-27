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
            .build()
            .context("building HTTP client")?;
        Ok(Self { http, registry: DEFAULT_REGISTRY.to_string() })
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

    /// Fetch the full package document (every published version).
    pub fn package_metadata(&self, name: &str) -> Result<PackageMetadata> {
        let url = format!("{}/{}", self.registry, encode_name(name));
        let resp = self.http.get(&url).send().with_context(|| format!("requesting {url}"))?;
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
