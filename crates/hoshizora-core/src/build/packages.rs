use std::path::Path;

use hoshizora_model::{Effects, FileFacts, Language, Node, NodeKind};

use super::Ctx;

/// Package directory for a file: the crate root for Rust (`crates/api/src/x.rs` → `crates/api`),
/// the top-level package dir for Python, the Go package directory, the nearest `src` parent or
/// first path segment otherwise.
pub(crate) fn package_dir(file: &FileFacts) -> String {
    let parts: Vec<&str> = file.path.split('/').collect();
    let dir = &parts[..parts.len().saturating_sub(1)];
    match file.language {
        Language::Go => dir.join("/"),
        Language::Python => {
            let segs = file.module.split('.').filter(|s| !s.is_empty()).count();
            if segs >= 1 && dir.len() >= segs - 1 {
                let keep = dir.len() + 2 - segs;
                if keep >= 1 && keep <= dir.len() {
                    return dir[..keep].join("/");
                }
            }
            generic(dir)
        }
        _ => generic(dir),
    }
}

fn generic(dir: &[&str]) -> String {
    if let Some(pos) = dir.iter().position(|s| *s == "src") {
        return dir[..pos].join("/");
    }
    dir.first().map(|s| (*s).to_owned()).unwrap_or_default()
}

pub(crate) fn build(ctx: &mut Ctx) {
    for file in ctx.files {
        let dir = package_dir(file);
        let name = if dir.is_empty() {
            ".".to_owned()
        } else {
            dir.rsplit('/').next().unwrap_or(&dir).to_owned()
        };
        let id = format!("mod:{}", if dir.is_empty() { "." } else { dir.as_str() });
        let idx = ctx.add_node(Node {
            id,
            kind: NodeKind::Package,
            name,
            qualified_name: if dir.is_empty() {
                ".".to_owned()
            } else {
                dir.clone()
            },
            language: Some(file.language),
            file: None,
            span: None,
            parent: None,
            function: None,
            effects: Effects::default(),
            entrypoint: None,
            dead: false,
            lines: 0,
        });
        ctx.nodes[idx].lines += file.lines;
        ctx.package_of_file.push(idx);
    }
}

/// Source files the manifest of a published JS package points at (`exports`, `main`, `module`,
/// `svelte`), for packages not marked private. Build output paths are mapped back to sources:
/// `dist/x.js` → `src/x`, `src/lib/x`.
pub(crate) fn library_entries(ctx: &Ctx, root: &Path) -> Vec<usize> {
    let mut dirs: Vec<String> = ctx
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.language, Language::TypeScript | Language::JavaScript))
        .map(|(fi, _)| ctx.nodes[ctx.package_of_file[fi]].qualified_name.clone())
        .collect();
    dirs.sort();
    dirs.dedup();
    let mut out = Vec::new();
    for dir in dirs {
        let base = if dir == "." {
            String::new()
        } else {
            format!("{dir}/")
        };
        let Ok(text) = std::fs::read_to_string(root.join(&base).join("package.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if manifest.get("private").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        let mut targets = Vec::new();
        for key in ["exports", "main", "module", "svelte"] {
            if let Some(v) = manifest.get(key) {
                string_leaves(v, &mut targets);
            }
        }
        for target in targets {
            let t = target.trim_start_matches("./");
            if t.ends_with(".d.ts") || !t.ends_with(['s', 'x']) {
                continue;
            }
            let stem = strip_ext(t);
            let mut keys = vec![stem.clone()];
            if let Some(rest) = stem.strip_prefix("dist/") {
                keys.push(format!("src/{rest}"));
                keys.push(format!("src/lib/{rest}"));
            }
            for key in keys {
                if let Some(files) = ctx.module_files.get(&format!("{base}{key}")) {
                    out.extend(files.iter().copied());
                    break;
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Functions named by Python packaging entry points (`[project.scripts]`,
/// `[tool.poetry.scripts]`, `[project.entry-points.*]`: `name = "pkg.cli:main"`).
pub(crate) fn python_entry_points(ctx: &Ctx, root: &Path) -> Vec<usize> {
    let mut dirs: Vec<&str> = Vec::new();
    for file in ctx.files.iter().filter(|f| f.language == Language::Python) {
        let mut dir = file.path.as_str();
        while let Some((parent, _)) = dir.rsplit_once('/') {
            dirs.push(parent);
            dir = parent;
        }
        dirs.push("");
    }
    dirs.sort_unstable();
    dirs.dedup();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(text) = std::fs::read_to_string(root.join(dir).join("pyproject.toml")) else {
            continue;
        };
        let mut in_scripts = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_scripts = matches!(
                    line,
                    "[project.scripts]" | "[project.gui-scripts]" | "[tool.poetry.scripts]"
                ) || line.starts_with("[project.entry-points");
                continue;
            }
            let Some((module, func)) = line
                .split_once('=')
                .filter(|_| in_scripts)
                .map(|(_, v)| v.trim().trim_matches(['"', '\'']))
                .and_then(|v| v.split_once(':'))
            else {
                continue;
            };
            let func = func.split_whitespace().next().unwrap_or(func);
            for &fi in ctx
                .module_files
                .get(&normalise(module))
                .into_iter()
                .flatten()
            {
                out.extend(ctx.fn_by_file_local[fi].get(func).copied());
            }
        }
    }
    out
}

fn string_leaves(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Object(map) => map.values().for_each(|v| string_leaves(v, out)),
        serde_json::Value::Array(items) => items.iter().for_each(|v| string_leaves(v, out)),
        _ => {}
    }
}

/// Keys under which a file can be found by an import: its module path with separators
/// normalised to `/`, and its path without extension (plus the directory for `index` files).
pub(crate) fn module_keys(file: &FileFacts) -> Vec<String> {
    let mut keys = Vec::new();
    if !file.module.is_empty() {
        keys.push(normalise(&file.module));
    }
    let no_ext = strip_ext(&file.path);
    keys.push(no_ext.clone());
    if let Some(dir) = no_ext
        .strip_suffix("/index")
        .or_else(|| no_ext.strip_suffix("/mod"))
    {
        keys.push(dir.to_owned());
    }
    if let Some(dir) = no_ext.strip_suffix("/__init__") {
        keys.push(dir.to_owned());
    }
    if file.language == Language::Go {
        let dir = no_ext.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        if !dir.is_empty() {
            keys.push(dir.to_owned());
        }
    }
    keys.dedup();
    keys
}

pub(crate) fn normalise(module: &str) -> String {
    module
        .replace("::", "/")
        .replace('.', "/")
        .trim_matches('/')
        .to_owned()
}

pub(crate) fn strip_ext(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => format!("{dir}/{stem}"),
            _ => path.to_owned(),
        },
        None => match path.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
            _ => path.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(path: &str, language: Language, module: &str) -> FileFacts {
        FileFacts {
            path: path.into(),
            language,
            module: module.into(),
            lines: 1,
            imports: vec![],
            symbols: vec![],
            domains: vec![],
        }
    }

    #[test]
    fn package_dirs() {
        assert_eq!(
            package_dir(&facts("crates/api/src/x.rs", Language::Rust, "crate::x")),
            "crates/api"
        );
        assert_eq!(
            package_dir(&facts("src/main.rs", Language::Rust, "crate")),
            ""
        );
        assert_eq!(
            package_dir(&facts("app/worker.py", Language::Python, "app.worker")),
            "app"
        );
        assert_eq!(
            package_dir(&facts("src/app/sub/x.py", Language::Python, "app.sub.x")),
            "src/app"
        );
        assert_eq!(
            package_dir(&facts("internal/db/db.go", Language::Go, "internal/db")),
            "internal/db"
        );
        assert_eq!(
            package_dir(&facts("webui/src/api.ts", Language::TypeScript, "src/api")),
            "webui"
        );
    }

    #[test]
    fn keys() {
        let k = module_keys(&facts(
            "webui/src/api/index.ts",
            Language::TypeScript,
            "src/api/index",
        ));
        assert!(k.contains(&"webui/src/api".to_owned()));
        assert!(k.contains(&"src/api/index".to_owned()));
    }
}
