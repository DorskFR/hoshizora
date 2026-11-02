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
        Ok(facts)
    }
}
