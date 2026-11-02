//! Table, read/write/delete and column-literal extraction from SQL strings with a tokenizer,
//! not a parser.

use hoshizora_model::EffectKind;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqlFacts {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub deletes: Vec<String>,
    /// `BEGIN` / `START TRANSACTION`.
    pub transaction: bool,
    /// `SELECT ... FOR UPDATE` / `FOR SHARE` / `LOCK TABLE`.
    pub locks: bool,
    /// Quoted literals assigned to a column by `SET col = 'x'` or `INSERT ... VALUES`.
    pub assignments: Vec<ColumnLiteral>,
    /// Quoted literals a column is compared against: `WHERE col = 'x'`, `col IN ('a', 'b')`.
    pub conditions: Vec<ColumnLiteral>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnLiteral {
    pub table: String,
    pub column: String,
    pub value: String,
}

impl SqlFacts {
    pub fn is_write(&self) -> bool {
        !self.writes.is_empty()
    }

    pub fn kind(&self) -> EffectKind {
        if self.is_write() {
            EffectKind::DbWrite
        } else if !self.deletes.is_empty() {
            EffectKind::DbDelete
        } else {
            EffectKind::DbRead
        }
    }

    /// Tables the statement changes, writes first.
    pub fn changed(&self) -> impl Iterator<Item = &String> {
        self.writes.iter().chain(&self.deletes)
    }
}

/// Returns `None` when the string does not look like SQL.
pub fn analyse(sql: &str) -> Option<SqlFacts> {
    let tokens = tokenize(sql);
    let first = tokens.first()?.to_ascii_uppercase();
    let mut facts = SqlFacts::default();
    let statement = match first.as_str() {
        "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "UPSERT" | "MERGE" | "REPLACE" => {
            first
        }
        "BEGIN" | "START" if tokens.len() <= 6 => {
            facts.transaction = true;
            return Some(facts);
        }
        "LOCK"
            if tokens
                .get(1)
                .is_some_and(|t| t.eq_ignore_ascii_case("TABLE")) =>
        {
            facts.locks = true;
            if let Some(name) = table_name(tokens.get(2).map(String::as_str)) {
                push(&mut facts.reads, name);
            }
            return Some(facts);
        }
        _ => return None,
    };
    let ctes: Vec<String> = tokens
        .windows(3)
        .filter(|w| w[1].eq_ignore_ascii_case("AS") && w[2] == "(")
        .map(|w| w[0].to_ascii_lowercase())
        .collect();
    let mut aliases: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;
    let mut i = 0;
    while i < tokens.len() {
        let upper = tokens[i].to_ascii_uppercase();
        let next = tokens.get(i + 1).map(|t| t.as_str());
        let after = tokens.get(i + 2).map(|t| t.as_str());
        match upper.as_str() {
            "FROM" | "JOIN" => {
                let next = if next.is_some_and(|t| t.eq_ignore_ascii_case("ONLY")) {
                    i += 1;
                    tokens.get(i + 1).map(|t| t.as_str())
                } else {
                    next
                };
                let after = tokens.get(i + 2).map(|t| t.as_str());
                if after != Some("(")
                    && let Some(name) = table_name(next)
                    && !ctes.contains(&name)
                {
                    let is_delete = statement == "DELETE" && upper == "FROM";
                    push(
                        if is_delete {
                            &mut facts.deletes
                        } else {
                            &mut facts.reads
                        },
                        name.clone(),
                    );
                    note_alias(&tokens, i + 2, &name, &mut aliases);
                    if is_delete {
                        current = Some(name);
                    }
                    i += 1;
                }
            }
            "INTO" => {
                if let Some(name) = table_name(next) {
                    push(&mut facts.writes, name.clone());
                    note_alias(&tokens, i + 2, &name, &mut aliases);
                    current = Some(name.clone());
                    if statement == "INSERT" {
                        insert_values(&tokens, i + 2, &name, &mut facts.assignments);
                    }
                    i += 1;
                }
            }
            "UPDATE" => {
                if after != Some("(")
                    && let Some(name) = table_name(next)
                    && !ctes.contains(&name)
                {
                    push(&mut facts.writes, name.clone());
                    note_alias(&tokens, i + 2, &name, &mut aliases);
                    current = Some(name);
                    i += 1;
                }
            }
            "MERGE" | "UPSERT" => {
                let target = if next.is_some_and(|t| t.eq_ignore_ascii_case("INTO")) {
                    i += 1;
                    tokens.get(i + 1).map(|t| t.as_str())
                } else {
                    next
                };
                if let Some(name) = table_name(target) {
                    push(&mut facts.writes, name.clone());
                    current = Some(name);
                    i += 1;
                }
            }
            "SET" => {
                if let Some(table) = &current {
                    set_assignments(&tokens, i + 1, table, &mut facts.assignments);
                }
            }
            "FOR"
                if next.is_some_and(|t| {
                    t.eq_ignore_ascii_case("UPDATE") || t.eq_ignore_ascii_case("SHARE")
                }) =>
            {
                facts.locks = true;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    let only = (facts.reads.len() + facts.writes.len() + facts.deletes.len() == 1)
        .then(|| facts.changed().chain(&facts.reads).next().cloned())
        .flatten();
    conditions(&tokens, &aliases, only.as_deref(), &mut facts.conditions);
    Some(facts)
}

/// `FROM users u` / `FROM users AS u` binds `u` to `users`.
fn note_alias(tokens: &[String], at: usize, table: &str, aliases: &mut Vec<(String, String)>) {
    let mut at = at;
    if tokens.get(at).is_some_and(|t| t.eq_ignore_ascii_case("AS")) {
        at += 1;
    }
    if let Some(alias) = tokens.get(at)
        && is_identifier(alias)
        && !KEYWORDS.contains(&alias.to_ascii_uppercase().as_str())
    {
        aliases.push((alias.to_ascii_lowercase(), table.to_owned()));
    }
}

fn is_identifier(t: &str) -> bool {
    !t.is_empty()
        && t != "("
        && t != ")"
        && t != "="
        && !t.starts_with('\'')
        && t.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '"' || c == '`')
}

fn literal(t: &str) -> Option<&str> {
    t.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))
}

fn column_name(t: &str) -> String {
    t.trim_matches(|c| c == '"' || c == '`')
        .to_ascii_lowercase()
}

/// `SET a = 'x', b = $1, c = 'y'` after an `UPDATE` or `ON CONFLICT DO UPDATE`.
fn set_assignments(tokens: &[String], from: usize, table: &str, out: &mut Vec<ColumnLiteral>) {
    let mut i = from;
    while i + 2 < tokens.len() {
        let col = &tokens[i];
        let upper = col.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "WHERE" | "RETURNING" | "FROM" | "ON" | "SELECT" | "INSERT" | "UPDATE"
        ) {
            break;
        }
        if is_identifier(col) && tokens[i + 1] == "=" {
            if let Some(v) = literal(&tokens[i + 2]) {
                out.push(ColumnLiteral {
                    table: table.to_owned(),
                    column: column_name(col.rsplit('.').next().unwrap_or(col)),
                    value: v.to_owned(),
                });
            }
            i += 3;
        } else {
            i += 1;
        }
    }
}

/// `INSERT INTO t (a, b) VALUES ('x', $1), ('y', $2)`.
fn insert_values(tokens: &[String], from: usize, table: &str, out: &mut Vec<ColumnLiteral>) {
    if tokens.get(from).map(String::as_str) != Some("(") {
        return;
    }
    let mut columns = Vec::new();
    let mut i = from + 1;
    while i < tokens.len() && tokens[i] != ")" {
        if !is_identifier(&tokens[i]) {
            return;
        }
        columns.push(column_name(&tokens[i]));
        i += 1;
    }
    i += 1;
    if !tokens
        .get(i)
        .is_some_and(|t| t.eq_ignore_ascii_case("VALUES"))
    {
        return;
    }
    i += 1;
    while tokens.get(i).map(String::as_str) == Some("(") {
        let mut depth = 0usize;
        let mut pos = 0usize;
        i += 1;
        while i < tokens.len() {
            match tokens[i].as_str() {
                "(" => depth += 1,
                ")" if depth == 0 => break,
                ")" => depth -= 1,
                t if depth == 0 => {
                    if let (Some(v), Some(col)) = (literal(t), columns.get(pos)) {
                        out.push(ColumnLiteral {
                            table: table.to_owned(),
                            column: col.clone(),
                            value: v.to_owned(),
                        });
                    }
                    pos += 1;
                }
                _ => {}
            }
            i += 1;
        }
        i += 1;
    }
}

/// `col = 'x'`, `col IN ('a', 'b')`, `col <> 'x'` anywhere in the statement. A qualified column
/// resolves through the aliases; a bare one belongs to the statement's only table.
fn conditions(
    tokens: &[String],
    aliases: &[(String, String)],
    only: Option<&str>,
    out: &mut Vec<ColumnLiteral>,
) {
    let resolve = |col: &str| -> Option<(String, String)> {
        let col = col.trim_matches(|c| c == '"' || c == '`');
        match col.rsplit_once('.') {
            Some((prefix, name)) => {
                let prefix = prefix.to_ascii_lowercase();
                aliases
                    .iter()
                    .find(|(a, t)| *a == prefix || *t == prefix)
                    .map(|(_, t)| (t.clone(), column_name(name)))
            }
            None => only.map(|t| (t.to_owned(), column_name(col))),
        }
    };
    let mut i = 0;
    while i < tokens.len() {
        let col = &tokens[i];
        if !is_identifier(col) || col.chars().all(|c| c.is_ascii_digit()) {
            i += 1;
            continue;
        }
        let upper = col.to_ascii_uppercase();
        if KEYWORDS.contains(&upper.as_str()) || upper == "SET" {
            i += 1;
            continue;
        }
        let Some(op) = tokens.get(i + 1) else { break };
        let mut values: Vec<&str> = Vec::new();
        let mut consumed = 1;
        if matches!(op.as_str(), "=" | "<>" | "!=") {
            if let Some(v) = tokens.get(i + 2).and_then(|t| literal(t)) {
                values.push(v);
                consumed = 3;
            }
        } else if op.eq_ignore_ascii_case("IN")
            && tokens.get(i + 2).map(String::as_str) == Some("(")
        {
            let mut j = i + 3;
            while j < tokens.len() && tokens[j] != ")" {
                match literal(&tokens[j]) {
                    Some(v) => values.push(v),
                    None => {
                        values.clear();
                        break;
                    }
                }
                j += 1;
            }
            consumed = j + 1 - i;
        } else if op.eq_ignore_ascii_case("NOT")
            && tokens
                .get(i + 2)
                .is_some_and(|t| t.eq_ignore_ascii_case("IN"))
            && tokens.get(i + 3).map(String::as_str) == Some("(")
        {
            let mut j = i + 4;
            while j < tokens.len() && tokens[j] != ")" {
                match literal(&tokens[j]) {
                    Some(v) => values.push(v),
                    None => {
                        values.clear();
                        break;
                    }
                }
                j += 1;
            }
            consumed = j + 1 - i;
        }
        if !values.is_empty()
            && !in_set_clause(tokens, i)
            && let Some((table, column)) = resolve(col)
        {
            for v in values {
                let lit = ColumnLiteral {
                    table: table.clone(),
                    column: column.clone(),
                    value: v.to_owned(),
                };
                if !out.contains(&lit) {
                    out.push(lit);
                }
            }
        }
        i += consumed.max(1);
    }
}

/// True between a `SET` and the next `WHERE`/`RETURNING`, where `col = 'x'` assigns.
fn in_set_clause(tokens: &[String], at: usize) -> bool {
    let mut j = at;
    while j > 0 {
        j -= 1;
        let u = tokens[j].to_ascii_uppercase();
        match u.as_str() {
            "SET" => return true,
            "WHERE" | "RETURNING" | "SELECT" | "FROM" | "ON" | "VALUES" | "AND" | "OR" | "WHEN"
            | "HAVING" => return false,
            _ => {}
        }
    }
    false
}

const KEYWORDS: &[&str] = &[
    "SELECT",
    "SET",
    "WHERE",
    "LATERAL",
    "UNNEST",
    "VALUES",
    "GENERATE_SERIES",
    "JSONB_ARRAY_ELEMENTS",
    "JSON_ARRAY_ELEMENTS",
    "JSONB_EACH",
    "REGEXP_SPLIT_TO_TABLE",
    "STRING_TO_TABLE",
    "ONLY",
    "AS",
    "ON",
    "USING",
    "RETURNING",
    "DEFAULT",
    "NOT",
    "EXISTS",
    "NULL",
    "TRUE",
    "FALSE",
    "DUAL",
    "CROSS",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "NATURAL",
    "ORDER",
    "GROUP",
    "LIMIT",
    "OFFSET",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "AND",
    "OR",
    "IN",
    "IS",
    "BY",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "DO",
    "CONFLICT",
    "NOTHING",
    "UPDATE",
    "INSERT",
    "INTO",
    "DELETE",
    "FROM",
    "JOIN",
    "FOR",
    "SHARE",
    "DISTINCT",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "ASC",
    "DESC",
    "HAVING",
    "COALESCE",
    "NULLIF",
    "CAST",
    "NOW",
];

fn table_name(token: Option<&str>) -> Option<String> {
    let t = token?;
    if t.is_empty()
        || t == "("
        || t == "="
        || t.starts_with('\'')
        || KEYWORDS.contains(&t.to_ascii_uppercase().as_str())
    {
        return None;
    }
    let name = t.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']');
    if name.len() <= 1
        || name.starts_with('$')
        || name.starts_with(':')
        || name.starts_with('?')
        || name.starts_with('%')
        || name.starts_with('@')
    {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '"' || c == '`')
    {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("pg_")
        || lower.starts_with("information_schema.")
        || lower.starts_with("pg_catalog.")
        || lower.starts_with("excluded.")
        || lower.starts_with("new.")
        || lower.starts_with("old.")
    {
        return None;
    }
    if let Some((prefix, _)) = lower.split_once('.')
        && prefix.len() <= 2
    {
        return None;
    }
    Some(lower)
}

fn push(list: &mut Vec<String>, name: String) {
    if !list.contains(&name) {
        list.push(name);
    }
}

/// Quoted literals stay one token with their quotes; `=`, `(` and `)` are tokens of their own.
fn tokenize(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                flush(&mut cur, &mut tokens);
                let mut s = String::from("'");
                while let Some(d) = chars.next() {
                    if d == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            s.push('\'');
                            continue;
                        }
                        break;
                    }
                    s.push(d);
                }
                s.push('\'');
                tokens.push(s);
            }
            '-' if chars.peek() == Some(&'-') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() || c == ',' || c == ';' => flush(&mut cur, &mut tokens),
            '(' | ')' | '=' => {
                flush(&mut cur, &mut tokens);
                tokens.push(c.to_string());
            }
            _ => cur.push(c),
        }
    }
    flush(&mut cur, &mut tokens);
    tokens
}

fn flush(cur: &mut String, tokens: &mut Vec<String>) {
    if !cur.is_empty() {
        tokens.push(std::mem::take(cur));
    }
}

#[cfg(test)]
mod tests {
    use hoshizora_model::EffectKind;

    use super::{ColumnLiteral, analyse};

    fn lit(table: &str, column: &str, value: &str) -> ColumnLiteral {
        ColumnLiteral {
            table: table.into(),
            column: column.into(),
            value: value.into(),
        }
    }

    #[test]
    fn select_with_join() {
        let f = analyse("SELECT u.id FROM users u JOIN sessions s ON s.user_id = u.id WHERE 1")
            .unwrap();
        assert_eq!(f.reads, vec!["users", "sessions"]);
        assert!(f.writes.is_empty());
        assert_eq!(f.kind(), EffectKind::DbRead);
    }

    #[test]
    fn insert_select_delete() {
        let f = analyse("INSERT INTO sessions (id) SELECT id FROM drafts").unwrap();
        assert_eq!(f.writes, vec!["sessions"]);
        assert_eq!(f.reads, vec!["drafts"]);
        let f = analyse("delete from \"events\" where id = $1").unwrap();
        assert_eq!(f.deletes, vec!["events"]);
        assert!(f.writes.is_empty());
        assert_eq!(f.kind(), EffectKind::DbDelete);
        let f = analyse("UPDATE users SET name = 'x' WHERE id = ?").unwrap();
        assert_eq!(f.writes, vec!["users"]);
        assert_eq!(f.kind(), EffectKind::DbWrite);
        assert!(analyse("http://x").is_none());
    }

    #[test]
    fn skips_keywords_functions_ctes_and_system_tables() {
        let f = analyse(
            "WITH RECURSIVE tree AS (SELECT id FROM rooms WHERE parent IS NULL UNION ALL SELECT r.id FROM rooms r JOIN tree t ON r.parent = t.id) \
             SELECT * FROM tree t JOIN LATERAL unnest($1::text[]) AS u(name) ON true JOIN generate_series(1, 3) g ON true \
             LEFT JOIN (SELECT 1) sub ON true JOIN pg_class c ON c.oid = 1 JOIN information_schema.columns ic ON true",
        )
        .unwrap();
        assert_eq!(f.reads, vec!["rooms"]);
        let f = analyse(
            "INSERT INTO labels (id, user_id) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET user_id = excluded.user_id RETURNING id",
        )
        .unwrap();
        assert_eq!(f.writes, vec!["labels"]);
        assert!(f.reads.is_empty());
        let f = analyse("DELETE FROM _sqlx_migrations WHERE version = $1").unwrap();
        assert_eq!(f.deletes, vec!["_sqlx_migrations"]);
        let f = analyse("SELECT 1 FROM r.user_id").unwrap();
        assert!(f.reads.is_empty());
    }

    #[test]
    fn transactions_and_row_locks() {
        let f = analyse("BEGIN").unwrap();
        assert!(f.transaction);
        assert!(analyse("START TRANSACTION").unwrap().transaction);
        assert!(!analyse("SELECT 1").unwrap().transaction);
        let f = analyse("SELECT * FROM jobs WHERE id = $1 FOR UPDATE SKIP LOCKED").unwrap();
        assert!(f.locks);
        assert_eq!(f.reads, vec!["jobs"]);
        assert!(!analyse("SELECT * FROM jobs").unwrap().locks);
        let f = analyse("LOCK TABLE jobs IN EXCLUSIVE MODE").unwrap();
        assert!(f.locks);
        assert_eq!(f.reads, vec!["jobs"]);
    }

    #[test]
    fn column_literals_written_and_compared() {
        let f = analyse(
            "UPDATE jobs SET status = 'done', finished_at = now(), note = $1 WHERE id = $2 AND status = 'running'",
        )
        .unwrap();
        assert_eq!(f.assignments, vec![lit("jobs", "status", "done")]);
        assert_eq!(f.conditions, vec![lit("jobs", "status", "running")]);

        let f = analyse(
            "INSERT INTO jobs (id, status, kind) VALUES ($1, 'queued', 'render'), ($2, 'queued', 'thumb') \
             ON CONFLICT (id) DO UPDATE SET status = 'requeued'",
        )
        .unwrap();
        assert_eq!(
            f.assignments,
            vec![
                lit("jobs", "status", "queued"),
                lit("jobs", "kind", "render"),
                lit("jobs", "status", "queued"),
                lit("jobs", "kind", "thumb"),
                lit("jobs", "status", "requeued"),
            ]
        );
        assert!(f.conditions.is_empty());

        let f = analyse(
            "SELECT j.id FROM jobs j JOIN users u ON u.id = j.user_id \
             WHERE j.status IN ('queued', 'running') AND u.role <> 'bot' AND j.kind = $1",
        )
        .unwrap();
        assert_eq!(
            f.conditions,
            vec![
                lit("jobs", "status", "queued"),
                lit("jobs", "status", "running"),
                lit("users", "role", "bot"),
            ]
        );

        let f = analyse("SELECT id FROM jobs WHERE status = 'it''s' AND x IN ($1, 'a')").unwrap();
        assert_eq!(f.conditions, vec![lit("jobs", "status", "it's")]);
        let f = analyse("SELECT a.x FROM a JOIN b ON a.id = b.a_id WHERE status = 'x'").unwrap();
        assert!(f.conditions.is_empty());
    }
}
