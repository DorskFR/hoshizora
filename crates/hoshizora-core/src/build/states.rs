//! Links each branch site to the value domain it discriminates on.
//!
//! A site names its domain when its values are qualified (`Status::Active` → `Status`); the
//! name is resolved within the language family, nearest first: same file, same package, then
//! the only domain of that name anywhere. Unqualified sites link by overlap: at least two
//! handled values, all of them members of exactly one domain (ties broken the same way).
//! A site whose values match none of the named domain's members stays unlinked.
//!
//! Columns come from the SQL seen in effects outside tests: every `table.column` some
//! statement sets to a quoted literal, with the statements comparing it against literals as
//! readers, linked to the domains (any language) whose members cover every value written.

use std::collections::{BTreeMap, HashMap};

use hoshizora_model::{
    ColumnReader, ColumnSite, ColumnValue, EntrypointKind, Language, StateColumn, StateDomain,
    StateSite, States,
};

use super::Ctx;
use crate::story::is_test_file;

fn family(language: Language) -> u8 {
    match language {
        Language::TypeScript | Language::JavaScript => 0,
        Language::Rust => 1,
        Language::Python => 2,
        Language::Go => 3,
    }
}

pub(crate) fn build(ctx: &mut Ctx) -> States {
    let mut domains: Vec<StateDomain> = Vec::new();
    let mut domain_file: Vec<usize> = Vec::new();
    let mut by_name: HashMap<(u8, &str), Vec<usize>> = HashMap::new();
    for (fi, file) in ctx.files.iter().enumerate() {
        for d in &file.domains {
            by_name
                .entry((family(file.language), d.name.as_str()))
                .or_default()
                .push(domains.len());
            domain_file.push(fi);
            domains.push(StateDomain {
                id: format!("domain:{}#{}", file.path, d.name),
                name: d.name.clone(),
                file: file.path.clone(),
                language: file.language,
                values: d.values.clone(),
                span: d.span,
            });
        }
    }

    let mut sites = Vec::new();
    let mut total = 0u32;
    for f in 0..ctx.fns.len() {
        let fi = ctx.fns[f].file;
        let file = &ctx.files[fi];
        let fam = family(file.language);
        let Some(body) = &ctx.symbol(f).body else {
            continue;
        };
        for d in &body.discriminations {
            total += 1;
            let candidates: Vec<usize> = match &d.domain_hint {
                Some(hint) => by_name
                    .get(&(fam, hint.as_str()))
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|&i| {
                        d.handled
                            .iter()
                            .any(|h| domains[i].values.iter().any(|v| v.matches(h)))
                    })
                    .collect(),
                None if d.handled.len() >= 2 => (0..domains.len())
                    .filter(|&i| {
                        family(domains[i].language) == fam
                            && d.handled
                                .iter()
                                .all(|h| domains[i].values.iter().any(|v| v.matches(h)))
                    })
                    .collect(),
                None => Vec::new(),
            };
            let Some(di) = nearest(ctx, fi, &candidates, &domain_file) else {
                continue;
            };
            let domain = &domains[di];
            let missing = domain
                .values
                .iter()
                .filter(|v| !d.handled.iter().any(|h| v.matches(h)))
                .map(|v| v.name.clone())
                .collect();
            sites.push(StateSite {
                function: ctx.nodes[ctx.fns[f].node].id.clone(),
                line: d.line,
                subject: d.subject.clone(),
                domain: domain.id.clone(),
                handled: d.handled.clone(),
                missing,
                default: d.default,
            });
        }
    }
    for _ in 0..total {
        ctx.count("states.sites");
    }
    for _ in 0..sites.len() {
        ctx.count("states.linked_sites");
    }
    domains.sort_by(|a, b| a.id.cmp(&b.id));
    sites.sort_by(|a, b| {
        (a.function.as_str(), a.line, a.subject.as_str()).cmp(&(
            b.function.as_str(),
            b.line,
            b.subject.as_str(),
        ))
    });
    let columns = columns(ctx, &domains);
    for _ in 0..columns.len() {
        ctx.count("states.columns");
    }
    States {
        domains,
        sites,
        columns,
    }
}

/// `(function, line)` → literal values one statement compares the column against.
type ReaderMap = BTreeMap<(String, u32), Vec<String>>;

fn columns(ctx: &Ctx, domains: &[StateDomain]) -> Vec<StateColumn> {
    let is_test = |f: usize| {
        let node = &ctx.nodes[ctx.fns[f].node];
        node.entrypoint == Some(EntrypointKind::Test)
            || is_test_file(&ctx.files[ctx.fns[f].file].path)
            || node
                .qualified_name
                .split("::")
                .any(|seg| seg == "tests" || seg == "test")
    };
    let mut written: BTreeMap<(String, String), BTreeMap<String, Vec<ColumnSite>>> =
        BTreeMap::new();
    for site in ctx.sql_sites.iter().filter(|s| !is_test(s.function)) {
        let function = ctx.nodes[ctx.fns[site.function].node].id.clone();
        for a in &site.assignments {
            let writers = written
                .entry((a.table.clone(), a.column.clone()))
                .or_default()
                .entry(a.value.clone())
                .or_default();
            let at = ColumnSite {
                function: function.clone(),
                line: site.line,
            };
            if !writers.contains(&at) {
                writers.push(at);
            }
        }
    }
    let mut readers: BTreeMap<(String, String), ReaderMap> = BTreeMap::new();
    for site in ctx.sql_sites.iter().filter(|s| !is_test(s.function)) {
        let function = ctx.nodes[ctx.fns[site.function].node].id.clone();
        for c in &site.conditions {
            let key = (c.table.clone(), c.column.clone());
            if !written.contains_key(&key) {
                continue;
            }
            let values = readers
                .entry(key)
                .or_default()
                .entry((function.clone(), site.line))
                .or_default();
            if !values.contains(&c.value) {
                values.push(c.value.clone());
            }
        }
    }
    written
        .into_iter()
        .map(|((table, column), values)| {
            let linked: Vec<String> = domains
                .iter()
                .filter(|d| {
                    values.keys().all(|v| {
                        d.values.iter().any(|m| {
                            m.literal.as_deref() == Some(v.as_str())
                                || m.name.eq_ignore_ascii_case(v)
                        })
                    })
                })
                .map(|d| d.id.clone())
                .collect();
            StateColumn {
                readers: readers
                    .remove(&(table.clone(), column.clone()))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|((function, line), values)| ColumnReader {
                        function,
                        line,
                        values,
                    })
                    .collect(),
                table,
                column,
                values: values
                    .into_iter()
                    .map(|(value, mut writers)| {
                        writers.sort();
                        ColumnValue { value, writers }
                    })
                    .collect(),
                domains: linked,
            }
        })
        .collect()
}

/// The single candidate closest to file `fi`; `None` when none or when the nearest tier
/// still holds several.
fn nearest(ctx: &Ctx, fi: usize, candidates: &[usize], domain_file: &[usize]) -> Option<usize> {
    let pick = |keep: &dyn Fn(usize) -> bool| -> Option<Option<usize>> {
        let tier: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&d| keep(domain_file[d]))
            .collect();
        match tier.as_slice() {
            [] => None,
            [one] => Some(Some(*one)),
            _ => Some(None),
        }
    };
    let pkg = ctx.package_of_file[fi];
    pick(&|f| f == fi)
        .or_else(|| pick(&|f| ctx.package_of_file[f] == pkg))
        .or_else(|| pick(&|_| true))
        .flatten()
}
