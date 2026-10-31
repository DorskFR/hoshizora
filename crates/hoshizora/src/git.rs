//! Shallow clones and updates of project repositories through the `git` binary.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use tokio::process::Command;

#[derive(Clone, Default)]
pub struct GitOptions {
    /// `HOSHIZORA_GIT_TOKEN`: sent as basic auth for `https://github.com/` URLs; never logged
    /// and never written into the clone's remote URL.
    pub token: Option<String>,
    pub timeout: Option<Duration>,
    /// Test hook: accept `file://` URLs, which the API always refuses.
    pub allow_file_urls: bool,
}

impl GitOptions {
    pub fn from_env() -> Self {
        Self {
            token: std::env::var("HOSHIZORA_GIT_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
            timeout: Some(Duration::from_secs(600)),
            allow_file_urls: false,
        }
    }
}

/// Accepts `https://host/...`, `ssh://...` and `git@host:owner/repo`; refuses anything that
/// `git` could read from the local filesystem or interpret as an option.
pub fn validate_url(url: &str, allow_file: bool) -> anyhow::Result<()> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') || url.chars().any(char::is_whitespace) {
        bail!("invalid git url");
    }
    let ok = url.starts_with("https://")
        || url.starts_with("ssh://")
        || (url.starts_with("git@") && url.contains(':'))
        || (allow_file && url.starts_with("file://"));
    if !ok {
        bail!("url must be https://, ssh:// or git@host:path");
    }
    Ok(())
}

pub fn validate_ref(r#ref: &str) -> anyhow::Result<()> {
    if r#ref.is_empty()
        || r#ref.starts_with('-')
        || r#ref.chars().any(|c| c.is_whitespace() || c == ':')
    {
        bail!("invalid git ref");
    }
    Ok(())
}

/// `owner-repo` of a git URL, lowercased and URL-safe.
pub fn slug_for_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, p)| p),
        None => url.split_once(':').map_or(url, |(_, p)| p),
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let tail = parts.len().saturating_sub(2);
    slugify(&parts[tail..].join("-"))
}

pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

fn command(opts: &GitOptions, url: Option<&str>) -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let (Some(token), Some(url)) = (&opts.token, url)
        && url.starts_with("https://github.com/")
    {
        let header = format!(
            "http.extraheader=Authorization: Basic {}",
            base64(format!("x-access-token:{token}").as_bytes())
        );
        cmd.arg("-c").arg(header);
    }
    cmd
}

async fn run(opts: &GitOptions, mut cmd: Command, what: &str) -> anyhow::Result<String> {
    let fut = cmd.output();
    let output = match opts.timeout {
        Some(t) => tokio::time::timeout(t, fut)
            .await
            .with_context(|| format!("git {what} timed out after {t:?}"))?,
        None => fut.await,
    }
    .with_context(|| format!("running git {what}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {what} failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub async fn clone(
    opts: &GitOptions,
    url: &str,
    r#ref: Option<&str>,
    dest: &Path,
) -> anyhow::Result<()> {
    validate_url(url, opts.allow_file_urls)?;
    if let Some(r) = r#ref {
        validate_ref(r)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut cmd = command(opts, Some(url));
    cmd.args(["clone", "--depth", "1", "--single-branch", "--quiet"]);
    if let Some(r) = r#ref {
        cmd.args(["--branch", r]);
    }
    cmd.arg("--").arg(url).arg(dest);
    tracing::info!(url, dest = %dest.display(), "cloning");
    run(opts, cmd, "clone").await.map(drop)
}

/// Fetches the latest commit of `ref` (or the cloned branch) and resets the checkout to it.
pub async fn update(
    opts: &GitOptions,
    url: &str,
    r#ref: Option<&str>,
    dest: &Path,
) -> anyhow::Result<()> {
    let mut cmd = command(opts, Some(url));
    cmd.current_dir(dest)
        .args(["fetch", "--depth", "1", "--quiet", "origin"]);
    if let Some(r) = r#ref {
        validate_ref(r)?;
        cmd.arg(r);
    }
    run(opts, cmd, "fetch").await?;
    let mut cmd = command(opts, None);
    cmd.current_dir(dest)
        .args(["reset", "--hard", "--quiet", "FETCH_HEAD"]);
    run(opts, cmd, "reset").await.map(drop)
}

pub async fn head(opts: &GitOptions, dest: &Path) -> anyhow::Result<String> {
    let mut cmd = command(opts, None);
    cmd.current_dir(dest).args(["rev-parse", "HEAD"]);
    run(opts, cmd, "rev-parse").await
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(
            slug_for_url("https://github.com/DorskFR/hoshizora"),
            "dorskfr-hoshizora"
        );
        assert_eq!(
            slug_for_url("https://github.com/DorskFR/hoshizora.git/"),
            "dorskfr-hoshizora"
        );
        assert_eq!(
            slug_for_url("git@github.com:tokio-rs/mini-redis.git"),
            "tokio-rs-mini-redis"
        );
        assert_eq!(slug_for_url("ssh://git@host/Owner/Re_po"), "owner-re-po");
        assert_eq!(slug_for_url("https://host/repo"), "repo");
        assert_eq!(slug_for_url("https://host/a/b/c.git"), "b-c");
        assert_eq!(slugify("///"), "project");
    }

    #[test]
    fn url_validation() {
        for ok in [
            "https://github.com/a/b",
            "ssh://git@github.com/a/b.git",
            "git@github.com:a/b.git",
        ] {
            validate_url(ok, false).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "file:///etc",
            "/etc/passwd",
            "./repo",
            "http://github.com/a/b",
            "-c core.foo=bar",
            "https://x y",
            "",
            "ext::sh -c 'id'",
        ] {
            assert!(validate_url(bad, false).is_err(), "{bad}");
        }
        validate_url("file:///tmp/x", true).unwrap();
        assert!(validate_ref("--upload-pack=x").is_err());
        assert!(validate_ref("main").is_ok());
    }

    #[test]
    fn base64_matches_rfc() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"x-access-token:tok"), "eC1hY2Nlc3MtdG9rZW46dG9r");
    }

    #[test]
    fn token_only_for_github_https() {
        let opts = GitOptions {
            token: Some("secret".into()),
            ..Default::default()
        };
        let github = command(&opts, Some("https://github.com/a/b"));
        assert!(
            github
                .as_std()
                .get_args()
                .any(|a| a.to_string_lossy().starts_with("http.extraheader="))
        );
        let other = command(&opts, Some("https://gitlab.com/a/b"));
        assert_eq!(other.as_std().get_args().count(), 0);
    }
}
