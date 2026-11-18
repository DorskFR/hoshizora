//! Svelte and Vue single-file components: `<script>` blocks parsed as TS/JS at their original lines.

use hoshizora_model::{FileFacts, Language};
use regex::Regex;
use std::sync::LazyLock;

use crate::Adapter;
use crate::common;
use crate::typescript::{Flavor, TsAdapter};

pub struct SfcAdapter {
    extension: &'static str,
}

impl SfcAdapter {
    pub const SVELTE: Self = Self {
        extension: "svelte",
    };
    pub const VUE: Self = Self { extension: "vue" };
}

static SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script\b([^>]*)>(.*?)</script\s*>").expect("script regex"));
static TEMPLATE_HANDLER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\s((?:on:|on|@|v-on:)[\w.|:-]+)\s*=\s*(?:\{([^}]*)\}|"([^"]*)"|'([^']*)')"#)
        .expect("handler regex")
});
static STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<style\b[^>]*>.*?</style\s*>").expect("style regex"));
static DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s(?:use|transition|in|out|animate):([A-Za-z_$][\w$.]*)").expect("directive regex")
});
static LANG_TS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"lang\s*=\s*["']?tsx?"#).expect("lang regex"));

/// Script blocks as `(flavor, source padded with newlines so lines match the component file)`.
pub fn script_blocks(source: &str) -> Vec<(Flavor, String)> {
    SCRIPT
        .captures_iter(source)
        .map(|cap| {
            let attrs = cap.get(1).map_or("", |m| m.as_str());
            let body = cap.get(2).expect("script body");
            let flavor = if LANG_TS.is_match(attrs) {
                Flavor::TypeScript
            } else {
                Flavor::JavaScript
            };
            let offset = source[..body.start()].matches('\n').count();
            (flavor, format!("{}{}", "\n".repeat(offset), body.as_str()))
        })
        .collect()
}

/// `(event, identifiers)` for every event attribute in the markup outside `<script>`/`<style>`.
pub fn template_handlers(source: &str) -> Vec<(String, Vec<String>)> {
    let markup = SCRIPT.replace_all(source, "");
    let markup = STYLE.replace_all(&markup, "");
    TEMPLATE_HANDLER
        .captures_iter(&markup)
        .filter_map(|cap| {
            let attr = cap.get(1)?.as_str();
            if attr.starts_with("on") && !attr.starts_with("on:") && attr.len() < 3 {
                return None;
            }
            let expr = cap
                .get(2)
                .or_else(|| cap.get(3))
                .or_else(|| cap.get(4))?
                .as_str();
            Some((common::event_name(attr)?, common::handler_idents(expr)))
        })
        .collect()
}

/// `source` with `<script>` and `<style>` blocks blanked, newlines kept.
fn markup(source: &str) -> String {
    let blank = |m: &regex::Captures| m[0].chars().filter(|c| *c == '\n').collect::<String>();
    let out = SCRIPT.replace_all(source, blank);
    STYLE.replace_all(&out, blank).into_owned()
}

/// The JavaScript expression a template tag evaluates: `{x}`, `{#if x}`, `{#each x as y}`,
/// `{:else if x}`, `{#await x then y}`, `{@const a = x}`, `{@render x}`, `{@attach x}`.
fn tag_expression(tag: &str) -> Option<&str> {
    let tag = tag.trim();
    let word = |prefix: &str| {
        tag.strip_prefix(prefix)
            .filter(|r| r.starts_with(char::is_whitespace))
            .map(str::trim)
    };
    if let Some(rest) = word("#each") {
        return Some(rest.split(" as ").next().unwrap_or(rest));
    }
    if let Some(rest) = word("#await") {
        let rest = rest.split(" then ").next().unwrap_or(rest);
        return Some(rest.split(" catch ").next().unwrap_or(rest));
    }
    if let Some(rest) = word("@const") {
        return rest.split_once('=').map(|(_, v)| v);
    }
    for prefix in [
        "#if", ":else if", "#key", "@html", "@render", "@attach", "@debug",
    ] {
        if let Some(rest) = word(prefix) {
            return Some(rest);
        }
    }
    if tag.starts_with(['#', ':', '/', '@']) {
        return None;
    }
    Some(tag.strip_prefix("...").unwrap_or(tag))
}

/// Template expressions and directive names as script statements, each on the line it sits on
/// in the component, so calls and references in the markup can be analysed like code.
pub fn template_script(source: &str) -> String {
    let markup = markup(source);
    let mut exprs: Vec<(usize, String)> = Vec::new();
    let bytes = markup.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut depth = 1;
        let mut quote: Option<u8> = None;
        let mut j = start;
        while j < bytes.len() && depth > 0 {
            let c = bytes[j];
            match quote {
                Some(_) if c == b'\\' => j += 1,
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    b'"' | b'\'' | b'`' => quote = Some(c),
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                },
            }
            j += 1;
        }
        if depth == 0
            && let Some(expr) = tag_expression(&markup[start..j - 1])
            && !expr.trim().is_empty()
        {
            exprs.push((start, expr.to_owned()));
        }
        i = j;
    }
    for cap in DIRECTIVE.captures_iter(&markup) {
        if let Some(m) = cap.get(1) {
            exprs.push((m.start(), m.as_str().to_owned()));
        }
    }
    exprs.sort_by_key(|(at, _)| *at);
    let mut out = String::new();
    let mut line = 0;
    for (at, expr) in exprs {
        let target = markup[..at].matches('\n').count();
        while line < target {
            out.push('\n');
            line += 1;
        }
        out.push('(');
        out.push_str(&expr);
        out.push_str(");");
        line += expr.matches('\n').count();
    }
    out
}

impl Adapter for SfcAdapter {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn extensions(&self) -> &[&str] {
        std::slice::from_ref(&self.extension)
    }

    fn parse(&self, rel_path: &str, source: &str, module: &str) -> anyhow::Result<FileFacts> {
        let blocks = script_blocks(source);
        let language = if blocks.iter().any(|(f, _)| matches!(f, Flavor::TypeScript)) {
            Language::TypeScript
        } else {
            Language::JavaScript
        };
        let mut facts = FileFacts {
            path: rel_path.to_string(),
            language,
            module: module.to_string(),
            lines: common::line_count(source),
            imports: vec![],
            symbols: vec![],
            domains: vec![],
        };
        for (flavor, script) in blocks {
            TsAdapter(flavor).parse_into(&script, &mut facts)?;
        }
        common::apply_ui_handlers(&mut facts.symbols, &template_handlers(source));
        if self.extension == "svelte" {
            let template =
                TsAdapter(Flavor::TypeScript).expression_body(&template_script(source))?;
            if let Some(m) = common::module_symbol(template, facts.lines) {
                common::merge_module_symbol(&mut facts.symbols, m);
            }
        }
        Ok(facts)
    }
}

#[cfg(test)]
mod tests {
    use super::template_script;

    #[test]
    fn template_expressions_keep_their_lines() {
        let source = "<script>\nlet a = 1;\n</script>\n{#each rows as row (row.id)}\n<a href={hrefOf(row)} use:tooltip {@attach focus}>{label(\"}\")}</a>\n{@const t = fmt(row)}\n{:else if ok}\n{/each}\n<p>don't {x}</p>\n";
        let script = template_script(source);
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(lines[3], "(rows);");
        assert_eq!(lines[4], "(hrefOf(row));(tooltip);(focus);(label(\"}\"));");
        assert_eq!(lines[5], "( fmt(row));");
        assert_eq!(lines[6], "(ok);");
        assert_eq!(lines[8], "(x);");
    }
}
