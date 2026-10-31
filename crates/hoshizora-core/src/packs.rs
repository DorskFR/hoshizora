//! Effect, route and client rules loaded from TOML packs.
//!
//! A pack is a TOML file with `[[effect]]` and `[[route]]` tables:
//!
//! ```toml
//! [[effect]]
//! id = "rust.sqlx.query"
//! match = ["sqlx::query*", "*.fetch_one", "*.fetch_all"]
//! requires_import = "sqlx"
//! kind = "db"          # db | db_read | db_write | network | fs_read | ... (db picks read/write from SQL)
//! target = "sql"       # sql | url | arg0 | none
//! language = "rust"    # optional
//! client = true        # url effects that should link to Route nodes
//! spawn = true         # closure calls become Spawns edges
//! async_only = true    # only applies inside async functions
//!
//! [[route]]
//! id = "rust.axum.route"
//! language = "rust"
//! style = "builder"    # attribute | builder
//! match = ["*.route"]
//! ```
//!
//! Patterns match the callee as written, after import aliases are expanded to the imported
//! path (`from sqlalchemy import text` makes `text(...)` match `sqlalchemy.text`). `*` matches
//! any run of characters.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Context;
use hoshizora_model::{EffectKind, Language};
use serde::Deserialize;

use crate::pattern::Pattern;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Read or write decided by the SQL statement; defaults to read.
    Db,
    Network,
    DbRead,
    DbWrite,
    FsRead,
    FsWrite,
    Process,
    Spawn,
    Join,
    Lock,
    Publish,
    Subscribe,
    Sleep,
    Blocking,
}

impl RuleKind {
    pub fn effect_kind(self) -> EffectKind {
        match self {
            Self::Db | Self::DbRead => EffectKind::DbRead,
            Self::Network => EffectKind::Network,
            Self::DbWrite => EffectKind::DbWrite,
            Self::FsRead => EffectKind::FsRead,
            Self::FsWrite => EffectKind::FsWrite,
            Self::Process => EffectKind::Process,
            Self::Spawn => EffectKind::Spawn,
            Self::Join => EffectKind::Join,
            Self::Lock => EffectKind::Lock,
            Self::Publish => EffectKind::Publish,
            Self::Subscribe => EffectKind::Subscribe,
            Self::Sleep => EffectKind::Sleep,
            Self::Blocking => EffectKind::Blocking,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    #[default]
    None,
    Sql,
    Url,
    Arg0,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EffectRule {
    pub id: String,
    #[serde(rename = "match")]
    pub patterns: Vec<Pattern>,
    pub kind: RuleKind,
    #[serde(default)]
    pub target: Target,
    #[serde(default)]
    pub requires_import: Option<String>,
    #[serde(default)]
    pub language: Option<Language>,
    #[serde(default)]
    pub client: bool,
    #[serde(default)]
    pub spawn: bool,
    #[serde(default)]
    pub async_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStyle {
    /// Attribute or decorator on the handler: `app.get("/x")`, `get("/x")`, `route(...)`.
    Attribute,
    /// Call that registers a path: `.route("/x", ...)`, `app.get("/x", h)`, `http.HandleFunc`.
    Builder,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteRule {
    pub id: String,
    pub style: RouteStyle,
    #[serde(rename = "match")]
    pub patterns: Vec<Pattern>,
    #[serde(default)]
    pub language: Option<Language>,
    /// Fixed HTTP method when the callee name is not a verb (`http.HandleFunc` → `ANY`).
    #[serde(default)]
    pub method: Option<String>,
}

/// Method names too common to resolve by name alone (`map`, `expect`, `push`...).
#[derive(Debug, Clone, Deserialize)]
pub struct ResolveRules {
    pub language: Language,
    #[serde(default)]
    pub ignore_methods: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PackFile {
    #[serde(default)]
    effect: Vec<EffectRule>,
    #[serde(default)]
    route: Vec<RouteRule>,
    #[serde(default)]
    resolve: Vec<ResolveRules>,
}

/// Effect, route and client rules loaded from TOML packs.
#[derive(Debug, Clone, Default)]
pub struct Packs {
    effects: BTreeMap<String, EffectRule>,
    routes: BTreeMap<String, RouteRule>,
    ignore_methods: BTreeMap<Language, HashSet<String>>,
}

const BUILTIN: &[(&str, &str)] = &[
    ("common.toml", include_str!("../../../packs/common.toml")),
    ("rust.toml", include_str!("../../../packs/rust.toml")),
    ("python.toml", include_str!("../../../packs/python.toml")),
    (
        "typescript.toml",
        include_str!("../../../packs/typescript.toml"),
    ),
    ("go.toml", include_str!("../../../packs/go.toml")),
];

impl Packs {
    /// Packs shipped in `packs/`, embedded at build time.
    pub fn builtin() -> Self {
        let mut packs = Self::default();
        for (name, text) in BUILTIN {
            packs
                .add_str(text)
                .unwrap_or_else(|e| panic!("builtin pack {name} is invalid: {e}"));
        }
        packs
    }

    /// Adds every `*.toml` pack in `dir`, overriding builtin rules with the same id.
    pub fn load_dir(&mut self, dir: &Path) -> anyhow::Result<()> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading pack dir {}", dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading pack {}", path.display()))?;
            self.add_str(&text)
                .with_context(|| format!("parsing pack {}", path.display()))?;
        }
        Ok(())
    }

    pub fn add_str(&mut self, text: &str) -> anyhow::Result<()> {
        let file: PackFile = toml::from_str(text)?;
        for rule in file.effect {
            self.effects.insert(rule.id.clone(), rule);
        }
        for rule in file.route {
            self.routes.insert(rule.id.clone(), rule);
        }
        for rules in file.resolve {
            self.ignore_methods
                .entry(rules.language)
                .or_default()
                .extend(rules.ignore_methods);
        }
        Ok(())
    }

    /// Whether `name` is on the language's stoplist of common methods. JavaScript shares the
    /// TypeScript list.
    pub fn ignores_method(&self, language: Language, name: &str) -> bool {
        let language = match language {
            Language::JavaScript => Language::TypeScript,
            l => l,
        };
        self.ignore_methods
            .get(&language)
            .is_some_and(|set| set.contains(name))
    }

    pub fn effects(&self) -> impl Iterator<Item = &EffectRule> {
        self.effects.values()
    }

    pub fn routes(&self) -> impl Iterator<Item = &RouteRule> {
        self.routes.values()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty() && self.routes.is_empty()
    }
}
