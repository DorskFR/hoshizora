//! Span → graph node joining, strongest method first.

use std::collections::HashMap;

use hoshizora_model::{JoinMethod, Node, NodeId, NodeKind, RuntimeSpan, SpanKind};

use crate::NodeLookup;

/// Lookup tables derived from the graph's synthetic nodes and file paths.
#[derive(Debug, Default, Clone)]
pub struct JoinCache {
    files: Vec<String>,
    routes: HashMap<(String, String), NodeId>,
    route_paths: HashMap<String, Vec<NodeId>>,
    tables: HashMap<String, NodeId>,
    topics: HashMap<String, NodeId>,
    hosts: HashMap<String, NodeId>,
}

impl JoinCache {
    pub fn new(nodes: &[Node]) -> Self {
        let mut cache = Self::default();
        let mut files = std::collections::BTreeSet::new();
        for node in nodes {
            if let Some(f) = &node.file {
                files.insert(normalise_path(f));
            }
            match node.kind {
                NodeKind::File => {
                    files.insert(normalise_path(synthetic_name(node)));
                }
                NodeKind::Route => {
                    let spec = synthetic_name(node);
                    let (method, path) = match spec.split_once(' ') {
                        Some((m, p)) => (m.to_ascii_uppercase(), normalise_route(p)),
                        None => (String::new(), normalise_route(spec)),
                    };
                    cache
                        .route_paths
                        .entry(path.clone())
                        .or_default()
                        .push(node.id.clone());
                    cache.routes.insert((method, path), node.id.clone());
                }
                NodeKind::Table => {
                    let name = synthetic_name(node).to_ascii_lowercase();
                    cache
                        .tables
                        .insert(last_segment(&name, '.').to_string(), node.id.clone());
                    cache.tables.insert(name, node.id.clone());
                }
                NodeKind::Topic => {
                    cache
                        .topics
                        .insert(synthetic_name(node).to_string(), node.id.clone());
                }
                NodeKind::Host => {
                    let host = synthetic_name(node).to_ascii_lowercase();
                    cache.hosts.insert(host.clone(), node.id.clone());
                    if let Some((h, _port)) = host.rsplit_once(':') {
                        cache.hosts.entry(h.to_string()).or_insert(node.id.clone());
                    }
                }
                _ => {}
            }
        }
        cache.files = files.into_iter().collect();
        cache
    }

    /// Graph file whose path is the longest suffix of `path` (absolute or crate-relative).
    pub fn resolve_file(&self, path: &str) -> Option<&str> {
        let path = normalise_path(path);
        let mut best: Option<&str> = None;
        for f in &self.files {
            if (path == *f || path.ends_with(&format!("/{f}")))
                && best.is_none_or(|b| f.len() > b.len())
            {
                best = Some(f);
            }
        }
        if best.is_some() {
            return best;
        }
        let mut reverse = self
            .files
            .iter()
            .filter(|f| f.ends_with(&format!("/{path}")))
            .map(String::as_str);
        let first = reverse.next()?;
        reverse.next().is_none().then_some(first)
    }

    pub fn route(&self, method: Option<&str>, route: &str) -> Option<&NodeId> {
        let path = normalise_route(route);
        if let Some(m) = method
            && let Some(id) = self.routes.get(&(m.to_ascii_uppercase(), path.clone()))
        {
            return Some(id);
        }
        if let Some(id) = self.routes.get(&(String::new(), path.clone())) {
            return Some(id);
        }
        if let Some([only]) = self.route_paths.get(&path).map(Vec::as_slice) {
            return Some(only);
        }
        let method = method.map(str::to_ascii_uppercase);
        let mut matches = self.routes.iter().filter(|((m, pattern), _)| {
            method
                .as_ref()
                .is_none_or(|wanted| m.is_empty() || m == wanted)
                && pattern_matches(pattern, &path)
        });
        let (_, first) = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    pub fn table(&self, name: &str) -> Option<&NodeId> {
        let name = name
            .trim_matches(|c| c == '"' || c == '`' || c == '\'' || c == '[' || c == ']')
            .to_ascii_lowercase();
        self.tables
            .get(&name)
            .or_else(|| self.tables.get(last_segment(&name, '.')))
    }

    pub fn topic(&self, name: &str) -> Option<&NodeId> {
        self.topics.get(name)
    }

    pub fn host(&self, host: &str) -> Option<&NodeId> {
        let host = host.to_ascii_lowercase();
        self.hosts
            .get(&host)
            .or_else(|| host.rsplit_once(':').and_then(|(h, _)| self.hosts.get(h)))
    }
}

/// `/stars/{}` matches `/stars/42`; a `{}` segment stands for exactly one concrete segment.
fn pattern_matches(pattern: &str, concrete: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').collect();
    let c: Vec<&str> = concrete.split('/').collect();
    p.len() == c.len() && p.iter().zip(&c).all(|(a, b)| *a == "{}" || a == b)
}

fn synthetic_name(node: &Node) -> &str {
    node.id
        .split_once(':')
        .map_or(node.name.as_str(), |(_, rest)| rest)
}

fn last_segment(s: &str, sep: char) -> &str {
    s.rsplit(sep).next().unwrap_or(s)
}

fn normalise_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    p.trim_start_matches("./").trim_end_matches('/').to_string()
}

/// `/users/:id`, `/users/{id}`, `/users/<int:id>`, `/users/*` → `/users/{}`.
pub fn normalise_route(route: &str) -> String {
    let route = route.split('?').next().unwrap_or(route);
    let segments: Vec<String> = route
        .split('/')
        .map(|seg| {
            if seg.starts_with(':')
                || seg.starts_with('{')
                || seg.starts_with('<')
                || seg.starts_with('*')
                || seg.starts_with('[')
            {
                "{}".to_string()
            } else {
                seg.to_string()
            }
        })
        .collect();
    let joined = segments.join("/");
    let trimmed = joined.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn attr<'a>(span: &'a RuntimeSpan, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| span.attrs.get(*k))
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}

/// Sets `span.node` and `span.join` to the first method that matches.
pub fn join(span: &mut RuntimeSpan, lookup: &dyn NodeLookup, cache: &JoinCache) {
    let server_route = (span.kind == SpanKind::Server && attr(span, &["http.route"]).is_some())
        .then(|| by_http_route(span, cache))
        .flatten();
    let found = server_route
        .or_else(|| by_code_location(span, lookup, cache))
        .or_else(|| by_code_function(span, lookup))
        .or_else(|| by_http_route(span, cache))
        .or_else(|| by_db_table(span, cache))
        .or_else(|| by_messaging(span, cache))
        .or_else(|| by_host(span, cache))
        .or_else(|| by_span_name(span, lookup));
    if let Some((node, method)) = found {
        span.node = Some(node);
        span.join = Some(method);
    }
}

/// Whether the span's `code.filepath` + line lands on a function of this graph.
pub fn locates(span: &RuntimeSpan, lookup: &dyn NodeLookup, cache: &JoinCache) -> bool {
    by_code_location(span, lookup, cache).is_some()
}

fn by_code_location(
    span: &RuntimeSpan,
    lookup: &dyn NodeLookup,
    cache: &JoinCache,
) -> Option<(NodeId, JoinMethod)> {
    let path = attr(span, &["code.file.path", "code.filepath"])?;
    let line: u32 = attr(span, &["code.line.number", "code.lineno"])?
        .parse()
        .ok()?;
    let file = cache.resolve_file(path)?;
    (line..=line + 2)
        .find_map(|l| lookup.function_at(file, l))
        .map(|n| (n.id.clone(), JoinMethod::CodeLocation))
}

fn by_code_function(span: &RuntimeSpan, lookup: &dyn NodeLookup) -> Option<(NodeId, JoinMethod)> {
    let name = attr(span, &["code.function.name", "code.function"])?;
    let namespace = attr(span, &["code.namespace"]);
    resolve_function(lookup, name, namespace).map(|id| (id, JoinMethod::CodeFunction))
}

/// `name` may be short (`create`) or qualified (`sessions.Store.create`, `Store::create`);
/// `namespace` is a module path like `cctui_server::api::sessions` or `app.worker`.
pub fn resolve_function(
    lookup: &dyn NodeLookup,
    name: &str,
    namespace: Option<&str>,
) -> Option<NodeId> {
    let qualified = match namespace {
        Some(ns) if !ns.is_empty() => Some(format!("{ns}::{name}")),
        _ => None,
    };
    if let Some(q) = &qualified
        && let [only] = lookup.functions_named(q).as_slice()
    {
        return Some(only.id.clone());
    }
    let mut candidates = lookup.functions_named(name);
    let mut ns_hint = namespace.map(str::to_string);
    if candidates.is_empty() {
        let (prefix, short) = split_qualified(name)?;
        candidates = lookup.functions_named(short);
        if candidates.is_empty() {
            return None;
        }
        ns_hint = Some(match namespace {
            Some(ns) if !ns.is_empty() => format!("{ns}::{prefix}"),
            _ => prefix.to_string(),
        });
    }
    if let [only] = candidates.as_slice() {
        return Some(only.id.clone());
    }
    let ns = ns_hint?;
    let segments: Vec<String> = ns
        .split([':', '.', '/'])
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('-', "_").to_ascii_lowercase())
        .collect();
    let mut best: Vec<(&Node, usize)> = candidates
        .iter()
        .map(|n| (*n, namespace_score(n, &segments)))
        .collect();
    best.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    match best.as_slice() {
        [(n, s), rest @ ..] if *s > 0 && rest.iter().all(|(_, o)| o < s) => Some(n.id.clone()),
        _ => None,
    }
}

fn split_qualified(name: &str) -> Option<(&str, &str)> {
    if let Some((prefix, short)) = name.rsplit_once("::") {
        return Some((prefix, short));
    }
    name.rsplit_once('.')
}

fn namespace_score(node: &Node, segments: &[String]) -> usize {
    let mut haystack: Vec<String> = Vec::new();
    if let Some(file) = &node.file {
        haystack.extend(file.split('/').map(|s| {
            s.split('.')
                .next()
                .unwrap_or(s)
                .replace('-', "_")
                .to_ascii_lowercase()
        }));
    }
    haystack.extend(
        node.qualified_name
            .split([':', '.', '/'])
            .filter(|s| !s.is_empty())
            .map(|s| s.replace('-', "_").to_ascii_lowercase()),
    );
    segments.iter().filter(|s| haystack.contains(s)).count()
}

/// Server spans match on `http.route`. Client spans whose URL path matches a route node
/// (browser `fetch` to our own API) match too, unless the host is a known `host:` node.
fn by_http_route(span: &RuntimeSpan, cache: &JoinCache) -> Option<(NodeId, JoinMethod)> {
    let method = attr(span, &["http.request.method", "http.method"]);
    let path = match span.kind {
        SpanKind::Server => attr(span, &["http.route"])?.to_string(),
        SpanKind::Client => {
            let url = attr(span, &["url.full", "http.url"])?;
            let host = attr(span, &["server.address", "net.peer.name"])
                .map(str::to_string)
                .or_else(|| url_host(url));
            if host.is_some_and(|h| !is_local_host(&h) && cache.host(&h).is_some()) {
                return None;
            }
            url_path(url)?
        }
        _ => return None,
    };
    cache
        .route(method, &path)
        .map(|id| (id.clone(), JoinMethod::HttpRoute))
}

fn is_local_host(host: &str) -> bool {
    let h = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    let h = h
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    h == "localhost"
        || h == "::1"
        || h == "0.0.0.0"
        || h.starts_with("127.")
        || h.ends_with(".localhost")
}

pub fn url_path(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path_start = rest.find('/')?;
    let path = &rest[path_start..];
    Some(path.split(['?', '#']).next().unwrap_or(path).to_string())
}

fn by_db_table(span: &RuntimeSpan, cache: &JoinCache) -> Option<(NodeId, JoinMethod)> {
    if let Some(name) = attr(
        span,
        &[
            "db.collection.name",
            "db.sql.table",
            "db.cassandra.table",
            "db.mongodb.collection",
        ],
    ) && let Some(id) = cache.table(name)
    {
        return Some((id.clone(), JoinMethod::DbTable));
    }
    let sql = attr(span, &["db.query.text", "db.statement"])?;
    sql_tables(sql)
        .iter()
        .find_map(|t| cache.table(t))
        .map(|id| (id.clone(), JoinMethod::DbTable))
}

/// Identifiers following FROM / JOIN / INTO / UPDATE / TABLE, in order of appearance.
pub fn sql_tables(sql: &str) -> Vec<String> {
    let tokens: Vec<&str> = sql
        .split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')' || c == ';')
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = Vec::new();
    for pair in tokens.windows(2) {
        let kw = pair[0].to_ascii_uppercase();
        if matches!(kw.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE") {
            let ident =
                pair[1].trim_matches(|c| c == '"' || c == '`' || c == '\'' || c == '[' || c == ']');
            let upper = ident.to_ascii_uppercase();
            if ident.is_empty()
                || matches!(
                    upper.as_str(),
                    "SELECT" | "ONLY" | "IF" | "LATERAL" | "WHERE" | "SET" | "VALUES"
                )
                || ident.starts_with('$')
                || ident.starts_with('?')
            {
                continue;
            }
            let name = ident.to_string();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

fn by_messaging(span: &RuntimeSpan, cache: &JoinCache) -> Option<(NodeId, JoinMethod)> {
    let topic = attr(
        span,
        &[
            "messaging.destination.name",
            "messaging.destination",
            "messaging.source.name",
        ],
    )?;
    cache
        .topic(topic)
        .map(|id| (id.clone(), JoinMethod::Messaging))
}

fn by_host(span: &RuntimeSpan, cache: &JoinCache) -> Option<(NodeId, JoinMethod)> {
    if span.kind != SpanKind::Client {
        return None;
    }
    let host = attr(span, &["server.address", "net.peer.name", "http.host"])
        .map(str::to_string)
        .or_else(|| attr(span, &["url.full", "http.url"]).and_then(url_host))?;
    cache.host(&host).map(|id| (id.clone(), JoinMethod::Host))
}

pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    (!host.is_empty()).then(|| host.to_string())
}

fn by_span_name(span: &RuntimeSpan, lookup: &dyn NodeLookup) -> Option<(NodeId, JoinMethod)> {
    let name = span.name.trim();
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    match lookup.functions_named(name).as_slice() {
        [only] => Some((only.id.clone(), JoinMethod::SpanName)),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use hoshizora_model::{Effects, Node, NodeKind, Span, SpanKind};

    use super::*;

    pub(crate) fn node(
        id: &str,
        kind: NodeKind,
        file: Option<&str>,
        span: Option<(u32, u32)>,
    ) -> Node {
        let name_part = id.split_once(':').map_or(id, |(_, r)| r);
        let name = name_part
            .rsplit(['#', ':', '.'])
            .next()
            .unwrap_or(name_part)
            .to_string();
        let qualified_name = name_part
            .split_once('#')
            .map_or(name_part, |(_, q)| q)
            .to_string();
        Node {
            id: id.to_string(),
            kind,
            name,
            qualified_name,
            language: None,
            file: file.map(str::to_string),
            span: span.map(|(s, e)| Span {
                start_line: s,
                end_line: e,
            }),
            parent: None,
            function: None,
            effects: Effects::default(),
            entrypoint: None,
            dead: false,
            lines: 0,
        }
    }

    pub(crate) struct FakeLookup {
        pub nodes: Vec<Node>,
    }

    impl NodeLookup for FakeLookup {
        fn node(&self, id: &str) -> Option<&Node> {
            self.nodes.iter().find(|n| n.id == id)
        }
        fn function_at(&self, file: &str, line: u32) -> Option<&Node> {
            self.nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Function && n.file.as_deref() == Some(file))
                .filter(|n| {
                    n.span
                        .is_some_and(|s| s.start_line <= line && line <= s.end_line)
                })
                .min_by_key(|n| n.span.map(|s| s.end_line - s.start_line))
        }
        fn functions_named(&self, name: &str) -> Vec<&Node> {
            self.nodes
                .iter()
                .filter(|n| {
                    n.kind == NodeKind::Function && (n.name == name || n.qualified_name == name)
                })
                .collect()
        }
        fn nodes(&self) -> &[Node] {
            &self.nodes
        }
    }

    pub(crate) fn graph() -> FakeLookup {
        FakeLookup {
            nodes: vec![
                node(
                    "file:crates/api/src/sessions.rs",
                    NodeKind::File,
                    None,
                    None,
                ),
                node(
                    "fn:crates/api/src/sessions.rs#SessionStore::create",
                    NodeKind::Function,
                    Some("crates/api/src/sessions.rs"),
                    Some((10, 30)),
                ),
                node(
                    "fn:crates/api/src/sessions.rs#SessionStore::list",
                    NodeKind::Function,
                    Some("crates/api/src/sessions.rs"),
                    Some((32, 40)),
                ),
                node(
                    "fn:crates/worker/src/jobs.rs#create",
                    NodeKind::Function,
                    Some("crates/worker/src/jobs.rs"),
                    Some((1, 5)),
                ),
                node(
                    "fn:crates/api/src/main.rs#main",
                    NodeKind::Function,
                    Some("crates/api/src/main.rs"),
                    Some((1, 50)),
                ),
                node("route:POST /api/sessions", NodeKind::Route, None, None),
                node("route:GET /api/sessions/{id}", NodeKind::Route, None, None),
                node("table:sessions", NodeKind::Table, None, None),
                node("topic:session.spawn", NodeKind::Topic, None, None),
                node("host:api.github.com", NodeKind::Host, None, None),
                node("host:localhost", NodeKind::Host, None, None),
            ],
        }
    }

    pub(crate) fn span(name: &str, kind: SpanKind, attrs: &[(&str, &str)]) -> RuntimeSpan {
        RuntimeSpan {
            trace_id: "t".into(),
            span_id: "s".into(),
            parent_span_id: None,
            service: "svc".into(),
            name: name.into(),
            kind,
            start_unix_nanos: 0,
            end_unix_nanos: 10,
            error: false,
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            node: None,
            join: None,
        }
    }

    fn joined(span: &mut RuntimeSpan) -> (Option<String>, Option<JoinMethod>) {
        let g = graph();
        let cache = JoinCache::new(&g.nodes);
        join(span, &g, &cache);
        (span.node.clone(), span.join)
    }

    #[test]
    fn code_location_absolute_and_relative_paths() {
        let mut s = span(
            "x",
            SpanKind::Internal,
            &[
                ("code.file.path", "/home/me/proj/crates/api/src/sessions.rs"),
                ("code.line.number", "12"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::create".into()),
                Some(JoinMethod::CodeLocation)
            )
        );
        let mut s = span(
            "x",
            SpanKind::Internal,
            &[("code.filepath", "src/sessions.rs"), ("code.lineno", "35")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::list".into()),
                Some(JoinMethod::CodeLocation)
            )
        );
        let mut s = span(
            "x",
            SpanKind::Internal,
            &[
                ("code.filepath", "crates/api/src/sessions.rs"),
                ("code.lineno", "8"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::create".into()),
                Some(JoinMethod::CodeLocation)
            )
        );
        let mut s = span(
            "x",
            SpanKind::Internal,
            &[
                ("code.filepath", "crates/api/src/sessions.rs"),
                ("code.lineno", "99"),
            ],
        );
        assert_eq!(joined(&mut s), (None, None));
    }

    #[test]
    fn code_function_with_namespace_disambiguation() {
        let mut s = span(
            "create",
            SpanKind::Internal,
            &[
                ("code.function.name", "create"),
                ("code.namespace", "cctui_api::sessions"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::create".into()),
                Some(JoinMethod::CodeFunction)
            )
        );
        let mut s = span(
            "create",
            SpanKind::Internal,
            &[
                ("code.function.name", "create"),
                ("code.namespace", "worker::jobs"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/worker/src/jobs.rs#create".into()),
                Some(JoinMethod::CodeFunction)
            )
        );
        let mut s = span(
            "x",
            SpanKind::Internal,
            &[("code.function.name", "SessionStore::create")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::create".into()),
                Some(JoinMethod::CodeFunction)
            )
        );
        let mut s = span(
            "x",
            SpanKind::Internal,
            &[("code.function.name", "sessions.SessionStore.create")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::create".into()),
                Some(JoinMethod::CodeFunction)
            )
        );
        let mut s = span("x", SpanKind::Internal, &[("code.function.name", "create")]);
        assert_eq!(joined(&mut s), (None, None));
    }

    #[test]
    fn http_route_on_server_spans_only() {
        let mut s = span(
            "POST /api/sessions",
            SpanKind::Server,
            &[
                ("http.route", "/api/sessions"),
                ("http.request.method", "POST"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:POST /api/sessions".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        let mut s = span(
            "GET",
            SpanKind::Server,
            &[("http.route", "/api/sessions/:id"), ("http.method", "GET")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:GET /api/sessions/{id}".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        let mut s = span(
            "GET",
            SpanKind::Server,
            &[("http.route", "/api/sessions/<id>")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:GET /api/sessions/{id}".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        let mut s = span(
            "POST /api/sessions",
            SpanKind::Client,
            &[
                ("http.route", "/api/sessions"),
                ("http.request.method", "POST"),
            ],
        );
        assert_eq!(joined(&mut s), (None, None));
    }

    #[test]
    fn db_table_from_attribute_and_sql() {
        let mut s = span(
            "q",
            SpanKind::Client,
            &[("db.collection.name", "public.sessions")],
        );
        assert_eq!(
            joined(&mut s),
            (Some("table:sessions".into()), Some(JoinMethod::DbTable))
        );
        let mut s = span(
            "q",
            SpanKind::Client,
            &[("db.query.text", "INSERT INTO \"sessions\" (id) VALUES ($1)")],
        );
        assert_eq!(
            joined(&mut s),
            (Some("table:sessions".into()), Some(JoinMethod::DbTable))
        );
        let mut s = span(
            "q",
            SpanKind::Client,
            &[(
                "db.statement",
                "select u.* from users u join sessions s on s.user_id = u.id",
            )],
        );
        assert_eq!(
            joined(&mut s),
            (Some("table:sessions".into()), Some(JoinMethod::DbTable))
        );
        assert_eq!(
            sql_tables("UPDATE sessions SET x = 1; DELETE FROM `logs` WHERE 1"),
            vec!["sessions", "logs"]
        );
        assert_eq!(
            sql_tables("SELECT * FROM (SELECT 1) t"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn messaging_host_and_span_name() {
        let mut s = span(
            "publish",
            SpanKind::Producer,
            &[("messaging.destination.name", "session.spawn")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("topic:session.spawn".into()),
                Some(JoinMethod::Messaging)
            )
        );
        let mut s = span(
            "GET",
            SpanKind::Client,
            &[("url.full", "https://user:pw@api.github.com:443/repos?x=1")],
        );
        assert_eq!(
            joined(&mut s),
            (Some("host:api.github.com".into()), Some(JoinMethod::Host))
        );
        let mut s = span(
            "GET",
            SpanKind::Client,
            &[("server.address", "API.GITHUB.COM")],
        );
        assert_eq!(
            joined(&mut s),
            (Some("host:api.github.com".into()), Some(JoinMethod::Host))
        );
        let mut s = span(
            "GET",
            SpanKind::Server,
            &[("server.address", "api.github.com")],
        );
        assert_eq!(joined(&mut s), (None, None));
        let mut s = span("main", SpanKind::Internal, &[]);
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/main.rs#main".into()),
                Some(JoinMethod::SpanName)
            )
        );
        let mut s = span("create", SpanKind::Internal, &[]);
        assert_eq!(joined(&mut s), (None, None));
    }

    #[test]
    fn stronger_method_wins_except_server_spans_with_a_route() {
        let mut s = span(
            "main",
            SpanKind::Internal,
            &[
                ("http.route", "/api/sessions"),
                ("http.request.method", "POST"),
                ("code.function.name", "list"),
                ("code.namespace", "api::sessions"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/sessions.rs#SessionStore::list".into()),
                Some(JoinMethod::CodeFunction)
            )
        );
        let mut s = span(
            "POST /api/sessions",
            SpanKind::Server,
            &[
                ("http.route", "/api/sessions"),
                ("http.request.method", "POST"),
                ("code.file.path", "crates/api/src/main.rs"),
                ("code.line.number", "10"),
                ("code.function.name", "main"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:POST /api/sessions".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        let mut s = span(
            "POST /nope",
            SpanKind::Server,
            &[("http.route", "/nope"), ("code.function.name", "main")],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("fn:crates/api/src/main.rs#main".into()),
                Some(JoinMethod::CodeFunction)
            )
        );
    }

    #[test]
    fn client_spans_to_own_routes_join_the_route_not_the_host() {
        let mut s = span(
            "HTTP POST",
            SpanKind::Client,
            &[
                ("http.method", "POST"),
                ("http.url", "http://localhost:8080/api/sessions"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:POST /api/sessions".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        let mut s = span(
            "GET",
            SpanKind::Client,
            &[
                ("http.request.method", "GET"),
                ("url.full", "http://api:8080/api/sessions/42?x=1"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:GET /api/sessions/{id}".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        let mut s = span(
            "POST",
            SpanKind::Client,
            &[
                ("http.method", "POST"),
                ("url.full", "https://api.github.com/api/sessions"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (Some("host:api.github.com".into()), Some(JoinMethod::Host))
        );
        let mut s = span(
            "GET",
            SpanKind::Client,
            &[
                ("http.method", "GET"),
                ("url.full", "http://localhost/unknown"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (Some("host:localhost".into()), Some(JoinMethod::Host))
        );
        let mut s = span(
            "GET",
            SpanKind::Client,
            &[
                ("http.method", "GET"),
                ("url.full", "http://127.0.0.1:8081/api/sessions/7"),
                ("server.address", "127.0.0.1"),
            ],
        );
        assert_eq!(
            joined(&mut s),
            (
                Some("route:GET /api/sessions/{id}".into()),
                Some(JoinMethod::HttpRoute)
            )
        );
        assert!(
            is_local_host("[::1]:8080")
                && is_local_host("LOCALHOST")
                && !is_local_host("api.github.com:443")
        );
        assert_eq!(url_path("http://h:1/a/b?q#f").as_deref(), Some("/a/b"));
        assert_eq!(url_path("http://h"), None);
    }

    #[test]
    fn route_normalisation() {
        assert_eq!(
            normalise_route("/a/:id/b/{x}/<int:y>/*"),
            "/a/{}/b/{}/{}/{}"
        );
        assert_eq!(normalise_route("a/b/"), "/a/b");
        assert_eq!(normalise_route("/"), "/");
    }
}
