//! Python calls that resolved to nothing but name project code: a method the enclosing class
//! does not define, a method missing from the class a field is named after, a name a project
//! module does not define, or a name no project function has.

use std::collections::{HashMap, HashSet};

use hoshizora_model::{CallSite, Language, SymbolKind, UnresolvedCall, UnresolvedScope};

use super::Ctx;
use super::analysis::is_test_file;
use super::resolve::{COMMON_METHODS, callee_segments, external, is_self, types_named};

/// Bases that add no methods a subclass would call on itself.
const INERT_BASES: &[&str] = &["object", "ABC", "abc.ABC", "Protocol", "typing.Protocol"];

/// Builtins the pack stoplist does not already name.
const BUILTINS: &[&str] = &[
    "ascii",
    "bin",
    "breakpoint",
    "bytearray",
    "bytes",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "delattr",
    "divmod",
    "eval",
    "exec",
    "frozenset",
    "globals",
    "help",
    "hex",
    "input",
    "locals",
    "memoryview",
    "object",
    "oct",
    "ord",
    "pow",
    "property",
    "quit",
    "slice",
    "staticmethod",
];

/// Python classes: project bases, subclasses, and which classes extend something outside the
/// project (whose inherited methods are unknown).
pub(crate) struct Hierarchy {
    bases: HashMap<usize, Vec<usize>>,
    children: HashMap<usize, Vec<usize>>,
    open: HashSet<usize>,
    /// Type node → (file, symbol).
    symbols: HashMap<usize, (usize, usize)>,
}

impl Hierarchy {
    pub(crate) fn new(ctx: &Ctx) -> Self {
        let mut h = Self {
            bases: HashMap::new(),
            children: HashMap::new(),
            open: HashSet::new(),
            symbols: HashMap::new(),
        };
        for (fi, file) in ctx.files.iter().enumerate() {
            if file.language != Language::Python {
                continue;
            }
            for (si, sym) in file.symbols.iter().enumerate() {
                if sym.kind != SymbolKind::Type {
                    continue;
                }
                let Some(&t) = ctx.type_by_file_local[fi].get(&sym.local_name) else {
                    continue;
                };
                h.symbols.insert(t, (fi, si));
                for base in &sym.implements {
                    if INERT_BASES.contains(&base.as_str())
                        || base.starts_with("Generic[")
                        || base.starts_with("metaclass=ABCMeta")
                    {
                        continue;
                    }
                    let short = base.rsplit('.').next().unwrap_or(base);
                    match types_named(ctx, fi, short).as_slice() {
                        [b] if !base.contains(['[', '=']) => {
                            h.bases.entry(t).or_default().push(*b);
                            h.children.entry(*b).or_default().push(t);
                        }
                        _ => {
                            h.open.insert(t);
                        }
                    }
                }
            }
        }
        h
    }

    fn closure(map: &HashMap<usize, Vec<usize>>, t: usize) -> Vec<usize> {
        let mut out = vec![t];
        let mut i = 0;
        while i < out.len() {
            for &n in map.get(&out[i]).into_iter().flatten() {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            i += 1;
        }
        out
    }

    /// Whether no instance of `t`, or of a subclass through dynamic dispatch, can answer
    /// `name`: no method, attribute or dynamic lookup anywhere in a closed hierarchy.
    fn lacks(&self, ctx: &Ctx, t: usize, name: &str) -> bool {
        Self::closure(&self.children, t).into_iter().all(|d| {
            Self::closure(&self.bases, d)
                .into_iter()
                .all(|a| !self.open.contains(&a) && !self.answers(ctx, a, name))
        })
    }

    fn answers(&self, ctx: &Ctx, ty: usize, name: &str) -> bool {
        let Some(&(fi, si)) = self.symbols.get(&ty) else {
            return true;
        };
        let sym = &ctx.files[fi].symbols[si];
        sym.params.iter().any(|p| p.name == name || p.name == "*")
            || ctx.files[fi].symbols.iter().any(|m| {
                m.parent.as_deref() == Some(sym.local_name.as_str())
                    && (m.name == name || m.name == "__getattr__" || m.name == "__getattribute__")
            })
    }
}

/// Initialiser of attribute `field` on the class or one of its project bases.
fn class_lineage_attribute(
    ctx: &Ctx,
    hier: &Hierarchy,
    class: usize,
    field: &str,
) -> Option<String> {
    Hierarchy::closure(&hier.bases, class)
        .into_iter()
        .find_map(|t| {
            let &(fi, si) = hier.symbols.get(&t)?;
            ctx.files[fi].symbols[si]
                .params
                .iter()
                .find(|p| p.name == field)?
                .default
                .clone()
        })
}

/// Normalised spelling for matching a field name to a class name: `_mouse` ↔ `Mouse`,
/// `nav_runner` ↔ `NavRunner`.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Class a field's initialiser constructs: `Tactics(pb, world) if pb else None` → `Tactics`.
fn constructed(init: &str) -> Option<&str> {
    let callee = init.split('(').next()?.trim();
    let last = callee.rsplit('.').next()?;
    (init.contains('(')
        && !last.is_empty()
        && last.chars().all(|c| c.is_alphanumeric() || c == '_')
        && last.chars().next().is_some_and(char::is_uppercase))
    .then_some(last)
}

pub(crate) fn classify(
    ctx: &Ctx,
    hier: &Hierarchy,
    f: usize,
    call: &CallSite,
) -> Option<UnresolvedCall> {
    let fi = ctx.fns[f].file;
    let file = &ctx.files[fi];
    if file.language != Language::Python || is_test_file(&file.path) || call.reference {
        return None;
    }
    if call.callee.contains(['(', '[', '"', '\'', '<', ' ']) {
        return None;
    }
    let segments = callee_segments(&call.callee);
    let name = segments.last()?.clone();
    if name.starts_with("__") || name.is_empty() {
        return None;
    }
    let sym = ctx.symbol(f);
    let unresolved = |scope, owner: Option<usize>| UnresolvedCall {
        function: ctx.nodes[ctx.fns[f].node].id.clone(),
        callee: call.callee.clone(),
        name: name.clone(),
        line: call.line,
        scope,
        owner: owner.map(|o| ctx.nodes[o].id.clone()),
    };
    let binding = segments
        .first()
        .and_then(|s| ctx.scopes[fi].bindings.get(s))
        .map(|&i| &ctx.scopes[fi].imports[i]);

    if segments.len() == 2
        && is_self(&segments[0])
        && let Some(class) = ctx.fns[f].parent_type
    {
        return hier
            .lacks(ctx, class, &name)
            .then(|| unresolved(UnresolvedScope::Own, Some(class)));
    }

    if segments.len() == 3
        && is_self(&segments[0])
        && let Some(class) = ctx.fns[f].parent_type
    {
        let field = &segments[1];
        let init = class_lineage_attribute(ctx, hier, class, field);
        let key = squash(field);
        let types: Vec<usize> = ctx
            .type_by_name
            .iter()
            .filter(|(n, _)| key.len() >= 3 && squash(n) == key)
            .flat_map(|(_, v)| v.iter().copied())
            .filter(|&t| {
                hier.symbols.get(&t).is_some_and(|&(tf, _)| {
                    !is_test_file(&ctx.files[tf].path)
                        && ctx.pkg_allowed[ctx.package_of_file[fi]]
                            .contains(&ctx.package_of_file[tf])
                })
            })
            .collect();
        let [t] = types.as_slice() else {
            return None;
        };
        let generic = ctx.packs.ignores_method(Language::Python, &name)
            || COMMON_METHODS.contains(&name.as_str());
        let evidence = match init.as_deref() {
            Some(init) => match constructed(init) {
                Some(c) => c == ctx.nodes[*t].name,
                None => !generic && init == field.as_str(),
            },
            None => false,
        };
        return (evidence && hier.lacks(ctx, *t, &name))
            .then(|| unresolved(UnresolvedScope::Field, Some(*t)));
    }

    if segments.len() == 2
        && let Some(imp) = binding
        && let Some(tf) = imp.file
    {
        let target = &ctx.files[tf];
        let stem = target
            .path
            .rsplit('/')
            .next()
            .and_then(|f| f.strip_suffix(".py"));
        let names_module =
            stem.is_some_and(|s| s != "__init__" && imp.path.rsplit(['.', '/']).next() == Some(s));
        if !names_module || target.language != Language::Python {
            return None;
        }
        let scope = &ctx.scopes[tf];
        let defined = target
            .symbols
            .iter()
            .any(|s| s.parent.is_none() && s.name == name)
            || target.symbols.iter().any(|s| {
                s.name == "<module>"
                    && s.body
                        .iter()
                        .flat_map(|b| &b.calls)
                        .any(|c| c.binds.contains(&name))
            })
            || scope.bindings.contains_key(&name)
            || !scope.globs.is_empty();
        return (!defined).then(|| unresolved(UnresolvedScope::Module, Some(ctx.file_nodes[tf])));
    }

    if binding.is_some_and(|imp| external(ctx, imp))
        || ctx.packs.ignores_method(Language::Python, &name)
        || COMMON_METHODS.contains(&name.as_str())
        || BUILTINS.contains(&name.as_str())
        || name.chars().next().is_some_and(char::is_uppercase)
        || (segments.len() == 1 && sym.params.iter().any(|p| p.name == name))
    {
        return None;
    }
    let exists = ctx.fn_by_name.get(&name).is_some_and(|v| {
        v.iter().any(|&c| {
            let cf = &ctx.files[ctx.fns[c].file];
            cf.language == Language::Python && !is_test_file(&cf.path)
        })
    });
    (!exists).then(|| unresolved(UnresolvedScope::Name, None))
}
