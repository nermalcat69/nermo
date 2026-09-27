use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(rename = "devDependencies", default)]
    pub dev_dependencies: BTreeMap<String, String>,
}

/// Walk up from `start` looking for a directory containing package.json.
pub fn find_project_root(start: &Path) -> Result<PathBuf> {
    let mut dir = start.canonicalize().context("resolving start directory")?;
    loop {
        if dir.join("package.json").is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            anyhow::bail!("no package.json found in {} or any parent directory", start.display());
        }
    }
}

pub fn load(project_root: &Path) -> Result<Manifest> {
    let path = project_root.join("package.json");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}
