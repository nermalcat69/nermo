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
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
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

/// Remove `names` from `dependencies` and `devDependencies`, editing the raw
/// JSON document (not round-tripping through the typed `Manifest`) so every
/// other field — `scripts`, `name`, anything nermo doesn't model — survives
/// untouched (PRD §13.4: "must preserve unrelated manifest fields"). Returns
/// which of the requested names were actually found and removed; a name not
/// present in either section is simply not in the returned list, not an
/// error, so removing something already gone is a harmless no-op.
pub fn remove_dependencies(project_root: &Path, names: &[String]) -> Result<Vec<String>> {
    let path = project_root.join("package.json");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let mut removed = Vec::new();
    for name in names {
        let mut found = false;
        for section in ["dependencies", "devDependencies"] {
            if let Some(obj) = doc.get_mut(section).and_then(|v| v.as_object_mut()) {
                found |= obj.remove(name.as_str()).is_some();
            }
        }
        if found {
            removed.push(name.clone());
        }
    }

    if !removed.is_empty() {
        let mut json = serde_json::to_string_pretty(&doc).context("serializing package.json")?;
        json.push('\n');
        std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(removed)
}

/// Add or update `packages` (name -> range, e.g. "^16.0.0") in
/// `dependencies` or `devDependencies`, editing the raw JSON document for
/// the same reason `remove_dependencies` does: every other field must
/// survive untouched. A name already present in the *other* section (e.g.
/// adding a regular dep that's currently a devDependency) is moved, not
/// duplicated.
pub fn add_dependencies(project_root: &Path, packages: &[(String, String)], dev: bool) -> Result<()> {
    let path = project_root.join("package.json");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let target_section = if dev { "devDependencies" } else { "dependencies" };
    let other_section = if dev { "dependencies" } else { "devDependencies" };

    for (name, range) in packages {
        if let Some(obj) = doc.get_mut(other_section).and_then(|v| v.as_object_mut()) {
            obj.remove(name.as_str());
        }
        let obj = doc
            .as_object_mut()
            .context("package.json is not a JSON object")?
            .entry(target_section)
            .or_insert_with(|| serde_json::Value::Object(Default::default()))
            .as_object_mut()
            .with_context(|| format!("{target_section} is not an object"))?;
        obj.insert(name.clone(), serde_json::Value::String(range.clone()));
    }

    let mut json = serde_json::to_string_pretty(&doc).context("serializing package.json")?;
    json.push('\n');
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_requested_names_and_preserves_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{
  "name": "demo",
  "scripts": { "build": "vite build" },
  "dependencies": { "react": "^19.0.0", "left-pad": "^1.3.0" },
  "devDependencies": { "typescript": "^5.8.0" }
}
"#,
        )
        .unwrap();

        let removed = remove_dependencies(dir.path(), &["left-pad".to_string()]).unwrap();
        assert_eq!(removed, vec!["left-pad".to_string()]);

        let manifest = load(dir.path()).unwrap();
        assert!(!manifest.dependencies.contains_key("left-pad"));
        assert!(manifest.dependencies.contains_key("react"));
        assert!(manifest.dev_dependencies.contains_key("typescript"));

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("package.json")).unwrap()).unwrap();
        assert_eq!(raw["scripts"]["build"], "vite build");
        assert_eq!(raw["name"], "demo");
    }

    #[test]
    fn removing_an_unknown_name_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let original = r#"{"name":"demo","dependencies":{"react":"^19.0.0"}}"#;
        std::fs::write(dir.path().join("package.json"), original).unwrap();

        let removed = remove_dependencies(dir.path(), &["does-not-exist".to_string()]).unwrap();
        assert!(removed.is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("package.json")).unwrap(), original);
    }

    #[test]
    fn add_dependencies_writes_new_entries_and_preserves_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{
  "name": "demo",
  "scripts": { "build": "vite build" },
  "dependencies": { "react": "^19.0.0" }
}
"#,
        )
        .unwrap();

        add_dependencies(dir.path(), &[("dotenv".to_string(), "^16.0.0".to_string())], false).unwrap();

        let manifest = load(dir.path()).unwrap();
        assert_eq!(manifest.dependencies.get("dotenv"), Some(&"^16.0.0".to_string()));
        assert_eq!(manifest.dependencies.get("react"), Some(&"^19.0.0".to_string()));

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("package.json")).unwrap()).unwrap();
        assert_eq!(raw["scripts"]["build"], "vite build");
        assert_eq!(raw["name"], "demo");
    }

    #[test]
    fn add_dependencies_with_dev_moves_an_existing_regular_dependency() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"demo","dependencies":{"typescript":"^5.0.0"}}"#,
        )
        .unwrap();

        add_dependencies(dir.path(), &[("typescript".to_string(), "^5.9.0".to_string())], true).unwrap();

        let manifest = load(dir.path()).unwrap();
        assert!(!manifest.dependencies.contains_key("typescript"));
        assert_eq!(manifest.dev_dependencies.get("typescript"), Some(&"^5.9.0".to_string()));
    }
}
