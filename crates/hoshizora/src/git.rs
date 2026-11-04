//! Shallow clones and updates of project repositories through the `git` binary.

use std::collections::BTreeMap;
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

/// Commit `ref` (a branch, tag or `HEAD`) points at on the remote, without a clone; `None`
/// when the remote has no such ref. Annotated tags resolve to the tagged commit.
pub async fn ls_remote(
    opts: &GitOptions,
    url: &str,
    r#ref: &str,
) -> anyhow::Result<Option<String>> {
    validate_url(url, opts.allow_file_urls)?;
    validate_ref(r#ref)?;
    let mut cmd = command(opts, Some(url));
    cmd.args(["ls-remote", "--quiet", "--"]).arg(url);
    if r#ref == "HEAD" {
        cmd.arg("HEAD");
    } else {
        cmd.arg(format!("refs/heads/{ref}", ref = r#ref))
            .arg(format!("refs/tags/{ref}", ref = r#ref));
    }
    let out = run(opts, cmd, "ls-remote").await?;
    Ok(parse_ls_remote(&out, r#ref))
}

fn parse_ls_remote(out: &str, r#ref: &str) -> Option<String> {
    let lines: Vec<(&str, &str)> = out
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(sha, name)| (sha.trim(), name.trim()))
        .collect();
    let pick = |name: &str| {
        lines
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(s, _)| (*s).to_string())
    };
    if r#ref == "HEAD" {
        return pick("HEAD");
    }
    pick(&format!("refs/heads/{ref}", ref = r#ref))
        .or_else(|| pick(&format!("refs/tags/{ref}^{{}}", ref = r#ref)))
        .or_else(|| pick(&format!("refs/tags/{ref}", ref = r#ref)))
}

pub async fn head(opts: &GitOptions, dest: &Path) -> anyhow::Result<String> {
    let mut cmd = command(opts, None);
    cmd.current_dir(dest).args(["rev-parse", "HEAD"]);
    run(opts, cmd, "rev-parse").await
}

/// Extracts `rev` of a local repository into `dest` with `git archive | tar`, without touching
/// the repository's worktree or index.
pub fn archive(repo: &Path, rev: &str, dest: &Path) -> anyhow::Result<()> {
    validate_ref(rev)?;
    std::fs::create_dir_all(dest)?;
    let mut git = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["archive", "--format=tar", rev])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git archive")?;
    let stdout = git.stdout.take().context("git archive stdout")?;
    let tar = std::process::Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(dest)
        .stdin(stdout)
        .stderr(Stdio::piped())
        .output()
        .context("running tar")?;
    let git = git.wait_with_output()?;
    if !git.status.success() {
        bail!(
            "git archive {rev} failed: {}",
            String::from_utf8_lossy(&git.stderr).trim()
        );
    }
    if !tar.status.success() {
        bail!(
            "extracting {rev} failed: {}",
            String::from_utf8_lossy(&tar.stderr).trim()
        );
    }
    Ok(())
}

/// Head-side line ranges (inclusive) of every hunk between two revisions, by file.
pub fn changed_lines(
    repo: &Path,
    base: &str,
    head: &str,
) -> anyhow::Result<BTreeMap<String, Vec<(u32, u32)>>> {
    validate_ref(base)?;
    validate_ref(head)?;
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--unified=0", "--no-color", base, head])
        .stdin(Stdio::null())
        .output()
        .context("running git diff")?;
    if !out.status.success() {
        bail!(
            "git diff {base} {head} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_hunks(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_hunks(diff: &str) -> BTreeMap<String, Vec<(u32, u32)>> {
    let mut out: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut file: Option<String> = None;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").map(str::to_owned);
        } else if let (Some(f), Some(rest)) = (&file, line.strip_prefix("@@ ")) {
            let Some(plus) = rest.split(' ').find_map(|w| w.strip_prefix('+')) else {
                continue;
            };
            let (start, count) = match plus.split_once(',') {
                Some((s, c)) => (s.parse().unwrap_or(0), c.parse().unwrap_or(0)),
                None => (plus.parse().unwrap_or(0), 1u32),
            };
            if count > 0 {
                out.entry(f.clone())
                    .or_default()
                    .push((start, start + count - 1));
            }
        }
    }
    out
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
    fn hunks_to_head_ranges() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3 +3,2 @@ fn x\n+a\n+b\n@@ -10,2 +12 @@\n-c\n+d\n@@ -20 +21,0 @@\n-e\ndiff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-x\n";
        let ranges = parse_hunks(diff);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges["src/a.rs"], vec![(3, 4), (12, 12)]);
    }

    #[test]
    fn ls_remote_picks_branch_then_peeled_tag() {
        let out = "aaa\trefs/heads/main\nbbb\trefs/tags/v1\nccc\trefs/tags/v1^{}\nddd\tHEAD\n";
        assert_eq!(parse_ls_remote(out, "main").as_deref(), Some("aaa"));
        assert_eq!(parse_ls_remote(out, "v1").as_deref(), Some("ccc"));
        assert_eq!(parse_ls_remote(out, "HEAD").as_deref(), Some("ddd"));
        assert_eq!(
            parse_ls_remote("bbb\trefs/tags/v2\n", "v2").as_deref(),
            Some("bbb")
        );
        assert_eq!(parse_ls_remote(out, "v3"), None);
        assert_eq!(parse_ls_remote("", "main"), None);
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
