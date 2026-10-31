//! Table and read/write extraction from SQL strings with a tokenizer, not a parser.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqlFacts {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}

impl SqlFacts {
    pub fn is_write(&self) -> bool {
        !self.writes.is_empty()
    }
}

/// Returns `None` when the string does not look like SQL.
pub fn analyse(sql: &str) -> Option<SqlFacts> {
    let tokens = tokenize(sql);
    let first = tokens.first()?.to_ascii_uppercase();
    let statement = match first.as_str() {
        "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "UPSERT" | "MERGE" | "REPLACE" => {
            first
        }
        _ => return None,
    };
    let ctes: Vec<String> = tokens
        .windows(3)
        .filter(|w| w[1].eq_ignore_ascii_case("AS") && w[2] == "(")
        .map(|w| w[0].to_ascii_lowercase())
        .collect();
    let mut facts = SqlFacts::default();
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
                            &mut facts.writes
                        } else {
                            &mut facts.reads
                        },
                        name,
                    );
                    i += 1;
                }
            }
            "INTO" => {
                if let Some(name) = table_name(next) {
                    push(&mut facts.writes, name);
                    i += 1;
                }
            }
            "UPDATE" => {
                if after != Some("(")
                    && let Some(name) = table_name(next)
                    && !ctes.contains(&name)
                {
                    push(&mut facts.writes, name);
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
                    push(&mut facts.writes, name);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Some(facts)
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
];

fn table_name(token: Option<&str>) -> Option<String> {
    let t = token?;
    if t.is_empty() || t == "(" || KEYWORDS.contains(&t.to_ascii_uppercase().as_str()) {
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

fn tokenize(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                for d in chars.by_ref() {
                    if d == '\'' {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() || c == ',' || c == ';' => flush(&mut cur, &mut tokens),
            '(' | ')' => {
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
    use super::analyse;

    #[test]
    fn select_with_join() {
        let f = analyse("SELECT u.id FROM users u JOIN sessions s ON s.user_id = u.id WHERE 1")
            .unwrap();
        assert_eq!(f.reads, vec!["users", "sessions"]);
        assert!(f.writes.is_empty());
    }

    #[test]
    fn insert_select_delete() {
        let f = analyse("INSERT INTO sessions (id) SELECT id FROM drafts").unwrap();
        assert_eq!(f.writes, vec!["sessions"]);
        assert_eq!(f.reads, vec!["drafts"]);
        let f = analyse("delete from \"events\" where id = $1").unwrap();
        assert_eq!(f.writes, vec!["events"]);
        let f = analyse("UPDATE users SET name = 'x' WHERE id = ?").unwrap();
        assert_eq!(f.writes, vec!["users"]);
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
        assert_eq!(f.writes, vec!["_sqlx_migrations"]);
        let f = analyse("SELECT 1 FROM r.user_id").unwrap();
        assert!(f.reads.is_empty());
    }
}
