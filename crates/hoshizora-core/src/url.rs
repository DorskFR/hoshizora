//! URL splitting and route path normalisation shared by linkers and route rules.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlParts {
    pub host: Option<String>,
    /// Normalised path, params replaced by `{}`.
    pub path: String,
}

pub fn parse(url: &str) -> Option<UrlParts> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        if host.is_empty() || host.contains("${") {
            return None;
        }
        return Some(UrlParts {
            host: Some(host),
            path: normalise_path(path),
        });
    }
    if url.starts_with('/') {
        return Some(UrlParts {
            host: None,
            path: normalise_path(url),
        });
    }
    if url.starts_with("${") || url.starts_with('{') {
        let rest = url.find('/').map(|i| &url[i..]).unwrap_or("/");
        return Some(UrlParts {
            host: None,
            path: normalise_path(rest),
        });
    }
    None
}

/// `/users/{id}`, `/users/:id`, `/users/<int:id>`, `/users/${id}` and `/users/*` all
/// become `/users/{}`. Query strings and trailing slashes are dropped.
pub fn normalise_path(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut out = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        out.push('/');
        if is_param(seg) {
            out.push_str("{}");
        } else {
            out.push_str(seg);
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn is_param(seg: &str) -> bool {
    seg.starts_with(':')
        || seg.starts_with('*')
        || seg.starts_with('{')
        || seg.starts_with('<')
        || seg.contains("${")
        || seg.contains('{')
}

pub const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

/// HTTP verb spelled by the last segment of a callee or decorator (`app.get`, `r.GET`).
pub fn method_from_name(name: &str) -> Option<&'static str> {
    let upper = name.to_ascii_uppercase();
    METHODS.iter().copied().find(|m| *m == upper)
}

#[cfg(test)]
mod tests {
    use super::{normalise_path, parse};

    #[test]
    fn paths() {
        assert_eq!(
            normalise_path("/users/{id}/posts/:post/"),
            "/users/{}/posts/{}"
        );
        assert_eq!(normalise_path("/items/<int:id>?x=1"), "/items/{}");
        assert_eq!(normalise_path("/api/sessions/${id}"), "/api/sessions/{}");
        assert_eq!(normalise_path(""), "/");
    }

    #[test]
    fn urls() {
        let u = parse("https://api.github.com:443/repos/x").unwrap();
        assert_eq!(u.host.as_deref(), Some("api.github.com"));
        assert_eq!(u.path, "/repos/x");
        let u = parse("/api/sessions").unwrap();
        assert!(u.host.is_none());
        assert_eq!(parse("{}/weather/{site}").unwrap().path, "/weather/{}");
        assert_eq!(parse("${API_URL}${path}").unwrap().path, "/");
        assert!(parse("not a url").is_none());
    }
}
