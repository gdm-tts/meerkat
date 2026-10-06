//! Thin wrapper around the `git` command line.
//!
//! Shelling out to `git` (instead of linking libgit2) means we automatically
//! honour the user's configuration: credential helpers, SSH agent, proxies,
//! `includeIf`, etc.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Maximum time a single `git fetch` may take before it is killed.
const FETCH_TIMEOUT: Duration = Duration::from_secs(90);
/// Maximum time for local (non-network) git commands.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tracking {
    /// Local branch and upstream point to the same commit.
    InSync,
    /// Local branch has commits that are not on the upstream.
    Ahead(u32),
    /// Upstream has commits that are not on the local branch.
    Behind(u32),
    /// Both sides have commits the other one lacks.
    Diverged { ahead: u32, behind: u32 },
    /// No upstream configured and no `origin/<branch>` found.
    NoUpstream,
    /// HEAD is detached, nothing to compare against.
    Detached,
}

#[derive(Clone, Debug)]
pub struct RepoStatus {
    pub branch: String,
    pub upstream: Option<String>,
    pub sync: Tracking,
    /// Working tree has uncommitted changes.
    pub dirty: bool,
    /// Error message from `git fetch`, if it failed (status is then based on
    /// the last known remote refs).
    pub fetch_error: Option<String>,
}

/// Fetch (optionally) and compute the sync state of the repository at `path`.
pub fn check(path: &Path, fetch: bool) -> Result<RepoStatus, String> {
    if !path.is_dir() {
        return Err("directory not found".into());
    }

    // Validates that this is a git work tree and gives a clean error otherwise.
    let inside = run(path, &["rev-parse", "--is-inside-work-tree"], LOCAL_TIMEOUT)
        .map_err(|e| first_line(&e))?;
    if inside.trim() != "true" {
        return Err("not a git work tree".into());
    }

    let fetch_error = if fetch {
        run(path, &["fetch", "--quiet", "--prune"], FETCH_TIMEOUT)
            .err()
            .map(|e| first_line(&e))
    } else {
        None
    };

    let branch = run(path, &["rev-parse", "--abbrev-ref", "HEAD"], LOCAL_TIMEOUT)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();

    let dirty = run(
        path,
        &["status", "--porcelain", "--untracked-files=normal"],
        LOCAL_TIMEOUT,
    )
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false);

    if branch.is_empty() || branch == "HEAD" {
        return Ok(RepoStatus {
            branch: if branch.is_empty() {
                "?".into()
            } else {
                "detached".into()
            },
            upstream: None,
            sync: Tracking::Detached,
            dirty,
            fetch_error,
        });
    }

    let upstream = find_upstream(path, &branch);
    let sync = match &upstream {
        None => Tracking::NoUpstream,
        Some(up) => {
            let range = format!("HEAD...{up}");
            let out = run(
                path,
                &["rev-list", "--left-right", "--count", &range],
                LOCAL_TIMEOUT,
            )
            .map_err(|e| first_line(&e))?;
            let mut it = out
                .split_whitespace()
                .map(|n| n.parse::<u32>().unwrap_or(0));
            let ahead = it.next().unwrap_or(0);
            let behind = it.next().unwrap_or(0);
            match (ahead, behind) {
                (0, 0) => Tracking::InSync,
                (a, 0) => Tracking::Ahead(a),
                (0, b) => Tracking::Behind(b),
                (a, b) => Tracking::Diverged {
                    ahead: a,
                    behind: b,
                },
            }
        }
    };

    Ok(RepoStatus {
        branch,
        upstream,
        sync,
        dirty,
        fetch_error,
    })
}

/// The configured upstream of the current branch, falling back to
/// `origin/<branch>` when no tracking branch is set.
fn find_upstream(path: &Path, branch: &str) -> Option<String> {
    if let Ok(up) = run(
        path,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
        LOCAL_TIMEOUT,
    ) {
        let up = up.trim();
        if !up.is_empty() {
            return Some(up.to_owned());
        }
    }
    let candidate = format!("refs/remotes/origin/{branch}");
    run(
        path,
        &["rev-parse", "--verify", "--quiet", &candidate],
        LOCAL_TIMEOUT,
    )
    .ok()
    .map(|_| format!("origin/{branch}"))
}

/// Run `git -C <path> <args>` and return stdout, or stderr on failure.
fn run(path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Never block waiting for a password prompt on a terminal we don't have.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Stable, untranslated output.
        .env("LC_ALL", "C");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;

    // Drain the pipes on helper threads so a chatty command can't deadlock.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} timed out", args[0]));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    };

    let out = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    if status.success() {
        Ok(out)
    } else if err.trim().is_empty() {
        Err(format!("git {} failed ({status})", args[0]))
    } else {
        Err(err)
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error")
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ")
        .to_owned()
}
