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
    let mut facts = SqlFacts::default();
    let mut i = 0;
    while i < tokens.len() {
        let upper = tokens[i].to_ascii_uppercase();
        let next = tokens.get(i + 1).map(|t| t.as_str());
        match upper.as_str() {
            "FROM" | "JOIN" => {
                if let Some(name) = table_name(next) {
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
                if let Some(name) = table_name(next) {
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

fn table_name(token: Option<&str>) -> Option<String> {
    let t = token?;
    if t.is_empty() || t == "(" || t == "SELECT" || t.eq_ignore_ascii_case("select") {
        return None;
    }
    let name = t.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']');
    if name.is_empty() || name.starts_with('$') || name.starts_with(':') || name.starts_with('?') {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '"' || c == '`')
    {
        return None;
    }
    Some(name.to_ascii_lowercase())
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
}
