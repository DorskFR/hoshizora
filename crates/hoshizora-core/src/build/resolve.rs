//! Import resolution and language-neutral call resolution.
//!
//! Resolution never crosses language families (Rust / Python / TS+JS / Go), never resolves
//! macros, and only searches the caller's package plus the packages it imports from. Method
//! names on a pack stoplist (`map`, `expect`, `push`...) are left unresolved unless an import
//! or the receiver type pins them down.

use std::collections::{HashMap, HashSet};

use hoshizora_model::{CallSite, Confidence, Edge, EdgeKind, FileFacts, Language, NodeKind};

use super::packages::normalise;
use super::{Ctx, FileScope, ImportTarget, effects};

/// Names matching more candidates than this are dropped rather than guessed.
pub(crate) const MAX_CANDIDATES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    Rust,
    Python,
    Script,
    Go,
}

pub(crate) fn family(language: Language) -> Family {
    match language {
        Language::Rust => Family::Rust,
        Language::Python => Family::Python,
        Language::TypeScript | Language::JavaScript => Family::Script,
        Language::Go => Family::Go,
    }
}

pub(crate) fn build_scopes(ctx: &mut Ctx) {
    let package_names: HashMap<String, usize> = ctx
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NodeKind::Package)
        .map(|(i, n)| (n.name.replace('-', "_"), i))
        .collect();

    for fi in 0..ctx.files.len() {
        let file = &ctx.files[fi];
        let mut scope = FileScope::default();
        for imp in &file.imports {
            let (path, glob) = match imp
                .path
                .strip_suffix("::*")
                .or_else(|| imp.path.strip_suffix(".*"))
            {
                Some(base) => (base.to_owned(), true),
                None => (imp.path.clone(), imp.names.iter().any(|n| n == "*")),
            };
            let files = resolve_import(ctx, fi, &path, &package_names);
            let dependency = if files.is_empty() {
                dependency_name(file, &path)
            } else {
                None
            };
            if files.is_empty() && dependency.is_none() {
                ctx.count("unresolved relative imports");
                if ctx.warnings.len() < 20 {
                    ctx.warnings
                        .push(format!("unresolved import `{}` in {}", imp.path, file.path));
                }
            }
            let dep_id = dependency.as_ref().map(|d| format!("ext:{d}"));
            let idx = scope.imports.len();
            scope.imports.push(ImportTarget {
                path: path.clone(),
                file: files.first().copied(),
                dependency: dep_id,
            });
            if glob {
                scope.globs.push(idx);
            }
            if let Some(alias) = &imp.alias {
                scope.bindings.insert(alias.clone(), idx);
            } else if !glob && let Some(last) = last_segment(&path) {
                scope.bindings.entry(last.to_owned()).or_insert(idx);
            }
            for name in &imp.names {
                if name == "*" {
                    continue;
                }
                let bound = last_segment(name).unwrap_or(name).to_owned();
                let sub = format!("{}/{}", normalise(&path), normalise(name));
                let sub_files = if files.is_empty()
                    || path.starts_with('.')
                    || file.language == Language::Python
                    || name.contains("::")
                {
                    resolve_import(ctx, fi, &sub, &package_names)
                } else {
                    Vec::new()
                };
                if let Some(&f) = sub_files.first() {
                    let sub_idx = scope.imports.len();
                    scope.imports.push(ImportTarget {
                        path: sub,
                        file: Some(f),
                        dependency: None,
                    });
                    scope.bindings.insert(bound, sub_idx);
                } else {
                    scope.bindings.insert(bound, idx);
                }
            }
        }
        ctx.scopes.push(scope);
    }

    let packages = ctx.nodes.len();
    let mut allowed: Vec<HashSet<usize>> = vec![HashSet::new(); packages];
    for fi in 0..ctx.files.len() {
        let pkg = ctx.package_of_file[fi];
        allowed[pkg].insert(pkg);
        for imp in &ctx.scopes[fi].imports {
            if let Some(tf) = imp.file {
                allowed[pkg].insert(ctx.package_of_file[tf]);
            }
        }
    }
    ctx.pkg_allowed = allowed;

    for fi in 0..ctx.files.len() {
        let from = ctx.file_nodes[fi];
        let imports = ctx.scopes[fi].imports.clone();
        for (i, imp) in imports.iter().enumerate() {
            let line = ctx.files[fi].imports.get(i).map(|x| x.line);
            if let Some(tf) = imp.file {
                let to = ctx.file_nodes[tf];
                if to != from {
                    let mut edge = Edge::new(
                        ctx.nodes[from].id.clone(),
                        ctx.nodes[to].id.clone(),
                        EdgeKind::Imports,
                        Confidence::Exact,
                    );
                    edge.line = line;
                    ctx.add_edge(from, to, edge);
                }
            } else if let Some(dep) = &imp.dependency {
                let name = dep.trim_start_matches("ext:").to_owned();
                let to = ctx.synthetic(NodeKind::Dependency, dep.clone(), name);
                let mut edge = Edge::new(
                    ctx.nodes[from].id.clone(),
                    ctx.nodes[to].id.clone(),
                    EdgeKind::Imports,
                    Confidence::Exact,
                );
                edge.line = line;
                ctx.add_edge(from, to, edge);
            }
        }
    }
}

fn last_segment(path: &str) -> Option<&str> {
    let seg = path.rsplit(['/', '.', ':']).find(|s| !s.is_empty())?;
    let seg = seg.strip_suffix(".ts").unwrap_or(seg);
    Some(seg)
}

/// Third-party package an unresolved import refers to, or `None` for relative/self paths.
fn dependency_name(file: &FileFacts, path: &str) -> Option<String> {
    match file.language {
        Language::Rust => {
            let first = path.split("::").next()?;
            match first {
                "crate" | "self" | "super" | "" => None,
                _ if !path.contains("::") => None,
                _ => Some(first.to_owned()),
            }
        }
        Language::Python => {
            if path.starts_with('.') {
                return None;
            }
            Some(path.split('.').next()?.to_owned())
        }
        Language::TypeScript | Language::JavaScript => {
            if path.starts_with('.')
                || path.starts_with('/')
                || path.starts_with('$')
                || path.starts_with('~')
                || path.starts_with("@/")
            {
                return None;
            }
            let path = path.strip_prefix("node:").unwrap_or(path);
            let mut parts = path.split('/');
            let first = parts.next()?;
            if first.starts_with('@') {
                Some(format!("{first}/{}", parts.next().unwrap_or("")))
            } else {
                Some(first.to_owned())
            }
        }
        Language::Go => {
            let first = path.split('/').next()?;
            if first.contains('.') {
                let parts: Vec<&str> = path.split('/').take(3).collect();
                Some(parts.join("/"))
            } else {
                Some(path.to_owned())
            }
        }
    }
}

/// Files under `key` or, failing that, under a prefix of it (`a/b/Item` → `a/b`).
fn probe(ctx: &Ctx, key: &str, restrict: Option<usize>) -> Vec<usize> {
    let mut probe = key.trim_matches('/').to_owned();
    loop {
        if let Some(files) = ctx.module_files.get(&probe) {
            let hits: Vec<usize> = files
                .iter()
                .copied()
                .filter(|&f| restrict.is_none_or(|p| ctx.package_of_file[f] == p))
                .collect();
            if !hits.is_empty() {
                return hits;
            }
        }
        match probe.rsplit_once('/') {
            Some((head, _)) if !head.is_empty() => probe = head.to_owned(),
            _ => return Vec::new(),
        }
    }
}

/// Files an import path refers to, empty when it points outside the repo.
fn resolve_import(
    ctx: &Ctx,
    fi: usize,
    path: &str,
    package_names: &HashMap<String, usize>,
) -> Vec<usize> {
    let file = &ctx.files[fi];
    let own_pkg = ctx.package_of_file[fi];
    let dir = file.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

    let mut keys: Vec<(String, Option<usize>)> = Vec::new();
    if path.starts_with("./") || path.starts_with("../") || path == "." || path == ".." {
        for key in join_relative(dir, path) {
            keys.push((key, None));
        }
    } else if file.language == Language::Python && path.starts_with('.') {
        let dots = path.chars().take_while(|c| *c == '.').count();
        let rest = &path[dots..];
        let mut base: Vec<&str> = file.module.split('.').filter(|s| !s.is_empty()).collect();
        let is_pkg_init = file.path.ends_with("__init__.py");
        let drop = if is_pkg_init { dots - 1 } else { dots };
        for _ in 0..drop {
            base.pop();
        }
        let mut key = base.join("/");
        if !rest.is_empty() {
            if !key.is_empty() {
                key.push('/');
            }
            key.push_str(&normalise(rest));
        }
        keys.push((key, Some(own_pkg)));
    } else if file.language == Language::Rust {
        let segs: Vec<&str> = path.split("::").collect();
        let module: Vec<&str> = file.module.split("::").filter(|s| !s.is_empty()).collect();
        let root = module.first().copied().unwrap_or("crate");
        match segs.first().copied() {
            Some("crate") => {
                let mut k = vec![root];
                k.extend(&segs[1..]);
                keys.push((normalise(&k.join("::")), Some(own_pkg)));
            }
            Some("self") => {
                let mut k = module.clone();
                k.extend(&segs[1..]);
                keys.push((normalise(&k.join("::")), Some(own_pkg)));
            }
            Some("super") => {
                let supers = segs.iter().take_while(|s| **s == "super").count();
                let rest = &segs[supers..];
                // `super` inside an inline `mod tests {}` names the file's own module, which the
                // adapter cannot tell apart from a `super` at file level: try both.
                for drop in [supers - 1, supers] {
                    let keep = module.len().saturating_sub(drop);
                    let mut k: Vec<&str> = module[..keep].to_vec();
                    k.extend(rest);
                    keys.push((normalise(&k.join("::")), Some(own_pkg)));
                }
            }
            Some(first) => {
                let direct = normalise(path);
                if ctx
                    .module_files
                    .contains_key(direct.split('/').next().unwrap_or(""))
                {
                    keys.push((direct, None));
                }
                let mut child = module.clone();
                child.extend(&segs);
                keys.push((normalise(&child.join("::")), Some(own_pkg)));
                if let Some(&pkg) = package_names.get(&first.replace('-', "_")) {
                    let mut k = vec!["crate"];
                    k.extend(&segs[1..]);
                    keys.push((normalise(&k.join("::")), Some(pkg)));
                    keys.push((normalise(&segs.join("::")), Some(pkg)));
                }
            }
            None => {}
        }
    } else if matches!(file.language, Language::TypeScript | Language::JavaScript)
        && let Some((alias, rest)) = path.split_once('/')
        && matches!(alias, "$lib" | "@" | "~" | "$app")
    {
        let pkg_dir = ctx.nodes[own_pkg].qualified_name.clone();
        let base = if pkg_dir == "." {
            String::new()
        } else {
            format!("{pkg_dir}/")
        };
        let sub = if alias == "$lib" { "src/lib/" } else { "src/" };
        for key in join_relative("", &format!("{base}{sub}{rest}")) {
            keys.push((key, Some(own_pkg)));
        }
    } else {
        keys.push((normalise(path), None));
    }

    for (key, restrict) in &keys {
        let hits = probe(ctx, key, *restrict);
        if !hits.is_empty() {
            return hits;
        }
    }
    if file.language == Language::Go
        && let Some((key, _)) = keys.first()
    {
        let mut segs: Vec<&str> = key.split('/').collect();
        while segs.len() > 1 {
            segs.remove(0);
            if let Some(files) = ctx.module_files.get(&segs.join("/")) {
                return files.clone();
            }
        }
    }
    Vec::new()
}

/// Candidate module keys for a relative import: as written (`x.svelte` for `x.svelte.ts`)
/// and without its source extension.
fn join_relative(dir: &str, rel: &str) -> Vec<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match joined.rsplit_once('.') {
        Some((stem, ext)) if SOURCE_EXTENSIONS.contains(&ext) && !stem.ends_with('/') => {
            vec![joined.clone(), stem.to_owned()]
        }
        _ => vec![joined],
    }
}

const SOURCE_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "svelte", "vue", "py", "rs", "go",
];

pub(crate) fn calls(ctx: &mut Ctx) {
    ctx.self_calls = vec![0; ctx.fns.len()];
    for f in 0..ctx.fns.len() {
        let sym = ctx.symbol(f);
        let Some(body) = &sym.body else { continue };
        let spawned = spawned_calls(ctx, f, &body.calls);
        let from = ctx.fns[f].node;
        for (ci, call) in body.calls.iter().enumerate() {
            let targets = resolve_call(ctx, f, call);
            let kind = if spawned[ci] {
                EdgeKind::Spawns
            } else {
                EdgeKind::Calls
            };
            let candidates = targets.len();
            for (t, confidence) in targets {
                if t == f {
                    ctx.self_calls[f] += 1;
                }
                let to = ctx.fns[t].node;
                let mut edge = Edge::new(
                    ctx.nodes[from].id.clone(),
                    ctx.nodes[to].id.clone(),
                    kind,
                    confidence,
                );
                edge.line = Some(call.line);
                edge.loop_depth = call.loop_depth;
                edge.conditional = call.conditional;
                edge.awaited = call.awaited;
                if let Some(receiver) = &call.receiver {
                    edge.attrs.insert("receiver".into(), receiver.clone());
                }
                if candidates > 1 {
                    edge.attrs
                        .insert("candidates".into(), candidates.to_string());
                }
                ctx.add_edge(from, to, edge);
            }
        }
    }
}

/// Calls handed to a spawn primitive: those on the spawn call's own line (`spawn(work())`)
/// and those inside a closure from that line up to the next call outside any closure.
fn spawned_calls(ctx: &Ctx, f: usize, calls: &[CallSite]) -> Vec<bool> {
    let mut spawned = vec![false; calls.len()];
    let is_spawn: Vec<bool> = calls
        .iter()
        .map(|c| effects::is_spawn_call(ctx, f, c))
        .collect();
    for (i, call) in calls.iter().enumerate() {
        if !is_spawn[i] {
            continue;
        }
        let end = calls
            .iter()
            .filter(|c| !c.in_closure && c.line > call.line)
            .map(|c| c.line)
            .min()
            .unwrap_or(u32::MAX);
        for (j, other) in calls.iter().enumerate() {
            if j == i || is_spawn[j] {
                continue;
            }
            let same_line = other.line == call.line;
            let in_window = other.in_closure && other.line >= call.line && other.line < end;
            if same_line || in_window {
                spawned[j] = true;
            }
        }
    }
    spawned
}

pub(crate) fn callee_segments(callee: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut cur = String::new();
    let chars: Vec<char> = callee.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && chars.get(i + 1) == Some(&':') => {
                out.push(std::mem::take(&mut cur));
                i += 2;
                continue;
            }
            '.' if depth == 0 => out.push(std::mem::take(&mut cur)),
            '!' if depth == 0 => {}
            _ if depth == 0 => cur.push(c),
            _ => {}
        }
        i += 1;
    }
    out.push(cur);
    out.retain(|s| !s.is_empty());
    out
}

struct Scope {
    file: usize,
    pkg: usize,
    family: Family,
}

/// Keeps candidates in the caller's language family (and, when `restrict` is set, in packages
/// the caller may see); drops the call when too many remain.
fn limited(
    ctx: &mut Ctx,
    scope: &Scope,
    cands: Vec<usize>,
    confidence: Confidence,
    restrict: bool,
) -> Vec<(usize, Confidence)> {
    let cands: Vec<usize> = cands
        .into_iter()
        .filter(|&c| {
            let fi = ctx.fns[c].file;
            family(ctx.files[fi].language) == scope.family
                && (!restrict || ctx.pkg_allowed[scope.pkg].contains(&ctx.package_of_file[fi]))
        })
        .collect();
    if cands.len() > MAX_CANDIDATES {
        ctx.count("ambiguous calls dropped (more than 3 candidates)");
        return Vec::new();
    }
    cands.into_iter().map(|c| (c, confidence)).collect()
}

fn in_file(ctx: &Ctx, tf: usize, segments: &[String], name: &str) -> Vec<usize> {
    for sep in ["::", "."] {
        let local = segments.join(sep);
        if let Some(&f) = ctx.fn_by_file_local[tf].get(&local) {
            return vec![f];
        }
    }
    ctx.fn_by_file_name[tf]
        .get(name)
        .cloned()
        .unwrap_or_default()
}

fn type_methods(ctx: &Ctx, ty: &str, name: &str) -> Option<Vec<usize>> {
    ctx.fn_by_type_method.get(ty)?.get(name).cloned()
}

fn is_self(segment: &str) -> bool {
    matches!(segment, "self" | "this" | "cls" | "Self")
}

pub(crate) fn resolve_call(ctx: &mut Ctx, f: usize, call: &CallSite) -> Vec<(usize, Confidence)> {
    if call.callee.ends_with('!') || call.name.ends_with('!') {
        return Vec::new();
    }
    let fi = ctx.fns[f].file;
    let language = ctx.files[fi].language;
    let scope = Scope {
        file: fi,
        pkg: ctx.package_of_file[fi],
        family: family(language),
    };
    let segments = callee_segments(&call.callee);
    let name = if call.name.is_empty() {
        segments.last().cloned().unwrap_or_default()
    } else {
        call.name.clone()
    };
    if name.is_empty() {
        return Vec::new();
    }
    let is_method = call.receiver.is_some()
        || (segments.len() >= 2
            && segments[0].chars().next().is_some_and(char::is_lowercase)
            && language != Language::Rust);

    // Qualified through an import, or an explicitly imported free name.
    if let Some(first) = segments.first()
        && let Some(&imp) = ctx.scopes[fi].bindings.get(first)
        && let Some(tf) = ctx.scopes[fi].imports[imp].file
    {
        let rest: Vec<String> = if segments.len() > 1 {
            segments[1..].to_vec()
        } else {
            segments.clone()
        };
        let cands = in_file(ctx, tf, &rest, &name);
        if !cands.is_empty() {
            return limited(ctx, &scope, cands, Confidence::Exact, false);
        }
    }

    // `self.method()` / `Self::method()` / `Type::method()` on the enclosing type.
    let sym = ctx.symbol(f);
    if let Some(parent) = ctx.fns[f].parent_type {
        let parent_name = ctx.nodes[parent].name.clone();
        let on_self = (segments.len() == 2
            && (is_self(&segments[0]) || segments[0] == parent_name))
            || call.receiver.as_deref().is_some_and(is_self);
        if on_self {
            let cands = in_file(ctx, scope.file, &[parent_name.clone(), name.clone()], "\0");
            if !cands.is_empty() {
                return limited(ctx, &scope, cands, Confidence::Exact, false);
            }
            if let Some(cands) = type_methods(ctx, &parent_name, &name) {
                return limited(ctx, &scope, cands, Confidence::Name, true);
            }
            return Vec::new();
        }
    }

    // Receiver whose type is a parameter type: `store.list()` with `store: State<SessionStore>`.
    if let Some(receiver) = &call.receiver
        && let Some(param) = sym.params.iter().find(|p| &p.name == receiver)
        && let Some(ty) = param.ty.as_deref().map(type_short_name)
        && let Some(cands) = type_methods(ctx, &ty, &name)
    {
        let confidence = if cands.len() == 1 {
            Confidence::Exact
        } else {
            Confidence::Name
        };
        return limited(ctx, &scope, cands, confidence, true);
    }

    // Path call `Type::method` / `module::function` without an import binding.
    if segments.len() >= 2 && !is_method {
        if let Some(&c) = ctx.fn_by_file_local[fi]
            .get(&segments.join("::"))
            .or_else(|| ctx.fn_by_file_local[fi].get(&segments.join(".")))
        {
            return vec![(c, Confidence::Exact)];
        }
        let qualifier = &segments[segments.len() - 2];
        if ctx.type_by_name.contains_key(qualifier)
            && let Some(cands) = type_methods(ctx, qualifier, &name)
        {
            return limited(ctx, &scope, cands, Confidence::Name, true);
        }
    }

    if let Some(cands) = ctx.fn_by_file_name[fi].get(&name).cloned() {
        let cands = prefer_sibling(ctx, f, cands);
        return limited(ctx, &scope, cands, Confidence::Name, false);
    }

    if (is_method || segments.len() == 1) && ctx.packs.ignores_method(language, &name) {
        return Vec::new();
    }

    if let Some(cands) = ctx
        .fn_by_package_name
        .get(&scope.pkg)
        .and_then(|m| m.get(&name))
        .cloned()
    {
        return limited(ctx, &scope, cands, Confidence::Name, false);
    }

    let globs = ctx.scopes[fi].globs.clone();
    for imp in globs {
        if let Some(tf) = ctx.scopes[fi].imports[imp].file
            && let Some(cands) = ctx.fn_by_file_name[tf].get(&name).cloned()
        {
            return limited(ctx, &scope, cands, Confidence::Name, false);
        }
    }

    if let Some(cands) = ctx.fn_by_name.get(&name).cloned() {
        let cands: Vec<usize> = cands
            .into_iter()
            .filter(|&c| ctx.pkg_allowed[scope.pkg].contains(&ctx.package_of_file[ctx.fns[c].file]))
            .collect();
        if is_method {
            return limited(ctx, &scope, cands, Confidence::Name, true);
        }
        if cands.len() == 1 {
            return limited(ctx, &scope, cands, Confidence::Name, true);
        }
        if cands.len() > 1 {
            ctx.count("ambiguous calls dropped (more than 3 candidates)");
        }
    }
    Vec::new()
}

fn prefer_sibling(ctx: &Ctx, f: usize, cands: Vec<usize>) -> Vec<usize> {
    if cands.len() <= 1 {
        return cands;
    }
    let parent = ctx.fns[f].parent_type;
    let siblings: Vec<usize> = cands
        .iter()
        .copied()
        .filter(|&c| ctx.fns[c].parent_type == parent)
        .collect();
    if siblings.len() == 1 { siblings } else { cands }
}

/// `&mut Option<Arc<SessionStore>>` → `SessionStore`; `*db.Store` → `Store`.
pub(crate) fn type_short_name(ty: &str) -> String {
    let inner = ty
        .trim_start_matches(['&', '*'])
        .trim_start_matches("mut ")
        .trim_start_matches("dyn ")
        .trim_start_matches("impl ")
        .trim();
    let inner = inner.strip_suffix(" | None").unwrap_or(inner);
    let mut best = inner;
    for wrapper in [
        "Option<",
        "Arc<",
        "Rc<",
        "Box<",
        "Optional[",
        "Promise<",
        "Result<",
        "State<",
        "Json<",
        "Extension<",
    ] {
        if let Some(rest) = best.strip_prefix(wrapper) {
            best = rest.trim_end_matches(['>', ']']);
        }
    }
    let best = best.split(['<', '[', ',', ' ']).next().unwrap_or(best);
    best.rsplit(['.', ':'])
        .next()
        .unwrap_or(best)
        .trim_end_matches('>')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{callee_segments, type_short_name};

    #[test]
    fn segments() {
        assert_eq!(callee_segments("sqlx::query"), vec!["sqlx", "query"]);
        assert_eq!(
            callee_segments("self.store.create"),
            vec!["self", "store", "create"]
        );
        assert_eq!(
            callee_segments("Vec::<u8>::with_capacity"),
            vec!["Vec", "with_capacity"]
        );
        assert_eq!(callee_segments("println!"), vec!["println"]);
    }

    #[test]
    fn types() {
        assert_eq!(type_short_name("&mut Arc<SessionStore>"), "SessionStore");
        assert_eq!(type_short_name("Option<db.Store>"), "Store");
        assert_eq!(type_short_name("*db.Store"), "Store");
        assert_eq!(type_short_name("Store"), "Store");
    }
}
