//! Module path of a file as its language spells it.

use std::path::{Path, PathBuf};

use hoshizora_model::Language;

pub fn module_path(root: &Path, rel: &str, language: Language) -> String {
    match language {
        Language::Rust => rust(root, rel),
        Language::Python => python(root, rel),
        Language::Go => go(root, rel),
        Language::TypeScript | Language::JavaScript => {
            if rel.ends_with(".svelte") || rel.ends_with(".vue") {
                rel.to_string()
            } else {
                strip_extension(rel)
            }
        }
    }
}

fn strip_extension(rel: &str) -> String {
    match rel.rfind('.') {
        Some(i) if !rel[i..].contains('/') => rel[..i].to_string(),
        _ => rel.to_string(),
    }
}

/// Nearest ancestor directory (inside `root`) containing `marker`, as a path relative to `root`.
fn nearest(root: &Path, rel: &str, marker: &str) -> Option<PathBuf> {
    let mut dir = Path::new(rel).parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if root.join(&d).join(marker).is_file() {
            return Some(d);
        }
        dir = d.parent().map(Path::to_path_buf);
        if d.as_os_str().is_empty() {
            break;
        }
    }
    None
}

fn rust(root: &Path, rel: &str) -> String {
    let (crate_dir, crate_name) = nearest(root, rel, "Cargo.toml")
        .and_then(|d| {
            let toml = std::fs::read_to_string(root.join(&d).join("Cargo.toml")).ok()?;
            let name = package_name(&toml)?;
            Some((d, name))
        })
        .unwrap_or_else(|| (PathBuf::new(), "crate".to_string()));
    let inner = Path::new(rel)
        .strip_prefix(&crate_dir)
        .unwrap_or(Path::new(rel))
        .with_extension("");
    let mut parts: Vec<String> = inner
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.first().is_some_and(|p| p == "src") {
        parts.remove(0);
    }
    if matches!(
        parts.last().map(String::as_str),
        Some("mod" | "lib" | "main")
    ) {
        parts.pop();
    }
    std::iter::once(crate_name.replace('-', "_"))
        .chain(parts)
        .collect::<Vec<_>>()
        .join("::")
}

fn package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package && let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

fn python(root: &Path, rel: &str) -> String {
    let path = Path::new(rel);
    let mut top: Option<PathBuf> = None;
    let mut dir = path.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d.as_os_str().is_empty() || !root.join(&d).join("__init__.py").is_file() {
            break;
        }
        top = Some(d.clone());
        dir = d.parent().map(Path::to_path_buf);
    }
    let base = top
        .and_then(|t| t.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let mut inner = path.strip_prefix(&base).unwrap_or(path).with_extension("");
    if base.as_os_str().is_empty()
        && let Ok(stripped) = inner.strip_prefix("src")
    {
        inner = stripped.to_path_buf();
    }
    let mut parts: Vec<String> = inner
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.last().is_some_and(|p| p == "__init__") {
        parts.pop();
    }
    parts.join(".")
}

fn go(root: &Path, rel: &str) -> String {
    let dir = Path::new(rel).parent().unwrap_or(Path::new(""));
    let Some(mod_dir) = nearest(root, rel, "go.mod") else {
        return dir.to_string_lossy().into_owned();
    };
    let module = std::fs::read_to_string(root.join(&mod_dir).join("go.mod"))
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("module ")
                    .map(|m| m.trim().to_string())
            })
        })
        .unwrap_or_default();
    let inner = dir
        .strip_prefix(&mod_dir)
        .unwrap_or(dir)
        .to_string_lossy()
        .into_owned();
    if inner.is_empty() {
        module
    } else {
        format!("{module}/{inner}")
    }
}
