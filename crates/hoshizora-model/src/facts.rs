//! What a language adapter extracts from one source file, before any cross-file resolution.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
}

/// 1-based, inclusive line range inside a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Span {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileFacts {
    /// Path relative to the indexed root, `/`-separated.
    pub path: String,
    pub language: Language,
    /// Module path as the language spells it: `crate::api::sessions`, `app.worker`, `src/api/client`.
    pub module: String,
    pub lines: u32,
    pub imports: Vec<Import>,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Import {
    /// Imported path as written: `sqlx::PgPool`, `requests`, `./api/client`, `net/http`.
    pub path: String,
    /// Name bound in this file, when it differs from the last path segment.
    pub alias: Option<String>,
    /// Names pulled in by `from x import a, b` / `import { a, b } from "x"` / `use x::{a, b}`.
    pub names: Vec<String>,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Type,
    Interface,
    Constant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    /// Name qualified within the file: `SessionStore::create`, `Worker.run`, `main`.
    pub local_name: String,
    /// `local_name` of the enclosing type or impl target, for methods.
    pub parent: Option<String>,
    pub signature: String,
    pub params: Vec<Param>,
    pub returns: Option<String>,
    pub doc: Option<String>,
    pub span: Span,
    pub is_async: bool,
    pub visibility: Visibility,
    /// Attributes, decorators and annotations as written, without the sigil:
    /// `tokio::main`, `test`, `app.get("/sessions")`, `router.post`.
    pub attributes: Vec<String>,
    /// Types this one implements or extends (`impl Trait for X`, `class X(Base)`, `implements I`).
    pub implements: Vec<String>,
    pub body: Option<Body>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: Option<String>,
    pub default: Option<String>,
}

/// Facts about a function body. Only present for functions and methods with a body.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Body {
    pub calls: Vec<CallSite>,
    /// `if`, `match`/`switch` arms beyond the first, `?:`, `&&`/`||` short circuits.
    pub branches: u32,
    pub loops: u32,
    pub max_loop_depth: u32,
    /// McCabe: 1 + decision points.
    pub cyclomatic: u32,
    /// Places that end the function early (`return`, `?`, `raise`, `throw`).
    pub early_exits: u32,
    /// Closures / lambdas / arrow functions defined inside the body.
    pub closures: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallSite {
    /// Callee as written, with whitespace removed: `sqlx::query`, `self.store.create`,
    /// `fetch`, `http.Get`, `asyncio.gather`, `tokio::spawn`.
    pub callee: String,
    /// Last segment of `callee`, the name used for resolution: `create`, `fetch`.
    pub name: String,
    /// Receiver expression for method calls: `self.store`, `client`, `db`.
    pub receiver: Option<String>,
    /// String literal arguments in order (unquoted), used for URLs, SQL, topics.
    pub string_args: Vec<String>,
    pub line: u32,
    /// Number of loops enclosing this call inside the function.
    pub loop_depth: u32,
    /// The call result is awaited (`.await`, `await x`, `yield from`).
    pub awaited: bool,
    /// The call sits inside a conditional branch.
    pub conditional: bool,
    /// The call is made inside a closure/lambda passed somewhere (spawned work, callbacks).
    pub in_closure: bool,
}
