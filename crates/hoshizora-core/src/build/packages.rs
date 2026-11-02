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
