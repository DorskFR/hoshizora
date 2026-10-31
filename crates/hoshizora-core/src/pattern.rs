use serde::Deserialize;

/// Glob-like pattern where `*` matches any run of characters.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub struct Pattern {
    source: String,
    parts: Vec<String>,
}

impl From<String> for Pattern {
    fn from(source: String) -> Self {
        let parts = source.split('*').map(str::to_owned).collect();
        Self { source, parts }
    }
}

impl Pattern {
    pub fn new(s: &str) -> Self {
        s.to_owned().into()
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn matches(&self, text: &str) -> bool {
        let n = self.parts.len();
        if n == 1 {
            return self.parts[0] == text;
        }
        let mut rest = text;
        for (i, part) in self.parts.iter().enumerate() {
            if i == 0 {
                match rest.strip_prefix(part.as_str()) {
                    Some(r) => rest = r,
                    None => return false,
                }
            } else if i == n - 1 {
                return rest.ends_with(part.as_str());
            } else if part.is_empty() {
                continue;
            } else {
                match rest.find(part.as_str()) {
                    Some(pos) => rest = &rest[pos + part.len()..],
                    None => return false,
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Pattern;

    #[test]
    fn globs() {
        assert!(Pattern::new("sqlx::query*").matches("sqlx::query_as"));
        assert!(Pattern::new("*.fetch_one").matches("sqlx::query(x).fetch_one"));
        assert!(Pattern::new("fetch").matches("fetch"));
        assert!(!Pattern::new("fetch").matches("window.fetch"));
        assert!(Pattern::new("tokio::*::spawn").matches("tokio::task::spawn"));
        assert!(!Pattern::new("tokio::*::spawn").matches("tokio::spawn"));
        assert!(Pattern::new("*").matches(""));
    }
}
