//! Links each branch site to the value domain it discriminates on.
//!
//! A site names its domain when its values are qualified (`Status::Active` → `Status`); the
//! name is resolved within the language family, nearest first: same file, same package, then
//! the only domain of that name anywhere. Unqualified sites link by overlap: at least two
//! handled values, all of them members of exactly one domain (ties broken the same way).
//! A site whose values match none of the named domain's members stays unlinked.

use std::collections::HashMap;

use hoshizora_model::{Language, StateDomain, StateSite, States};

use super::Ctx;

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
    States { domains, sites }
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
