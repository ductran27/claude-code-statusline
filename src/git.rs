//! Branch, dirty flag and upstream distance from one `git status` call,
//! cached briefly so a burst of renders costs one git run.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, UNIX_EPOCH};

/// A slow repository (network drive, huge untracked tree) must not stall
/// the status line; past this the branch is read from HEAD instead.
const TIMEOUT: Duration = Duration::from_millis(1500);
/// A cached result is reused while the repository metadata is unchanged
/// and it is younger than this. Commits, staging, checkouts and branch
/// switches change the metadata and show at once; edits to tracked or new
/// files show within this many seconds.
const TTL_SECS: i64 = 5;
/// When git failed or timed out, wait this long before trying again.
const SLOW_TTL_SECS: i64 = 60;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Git {
    pub branch: String,
    /// `None` when git could not run (timeout, dubious ownership, no git).
    pub dirty: Option<bool>,
    pub ahead: u64,
    pub behind: u64,
}

/// Nearest directory at or above `start` holding a `.git` dir or file.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The git directory of a work tree, following a worktree's `gitdir:` file.
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    let dotgit = root.join(".git");
    if dotgit.is_dir() {
        return Some(dotgit);
    }
    let text = fs::read_to_string(&dotgit).ok()?;
    let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    Some(if p.is_absolute() { p } else { root.join(p) })
}

fn mtime_ns(p: &Path) -> u128 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// Changes whenever git rewrites the index, HEAD, packed refs or adds or
/// renames a file in the git directory (every ref and index update does).
fn stamp(gitdir: &Path) -> String {
    let m = |name: &str| mtime_ns(&gitdir.join(name));
    format!(
        "{}:{}:{}:{}",
        mtime_ns(gitdir),
        m("index"),
        m("HEAD"),
        m("packed-refs")
    )
}

/// FNV-1a, for stable cache file names.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn encode(key: &str, at: i64, slow: bool, g: &Git) -> String {
    let dirty = match g.dirty {
        None => "-",
        Some(true) => "1",
        Some(false) => "0",
    };
    format!(
        "{key}\n{at}\n{}\n{dirty}\n{}\n{}\n{}\n",
        g.branch,
        g.ahead,
        g.behind,
        u8::from(slow)
    )
}

fn decode(text: &str) -> Option<(String, i64, bool, Git)> {
    let mut l = text.lines();
    let key = l.next()?.to_string();
    let at = l.next()?.parse().ok()?;
    let branch = l.next()?.to_string();
    let dirty = match l.next()? {
        "1" => Some(true),
        "0" => Some(false),
        "-" => None,
        _ => return None,
    };
    let ahead = l.next()?.parse().ok()?;
    let behind = l.next()?.parse().ok()?;
    let slow = l.next()? == "1";
    (!branch.is_empty()).then_some((
        key,
        at,
        slow,
        Git {
            branch,
            dirty,
            ahead,
            behind,
        },
    ))
}

pub fn status(dir: &Path, cache_dir: Option<&Path>, now: i64) -> Option<Git> {
    let root = find_root(dir)?;
    let gitdir = git_dir(&root)?;
    let key = stamp(&gitdir);
    let file = cache_dir.map(|c| c.join(format!("git-{:016x}", fnv(root.to_string_lossy().as_bytes()))));
    if let Some((k, at, slow, g)) = file
        .as_ref()
        .and_then(|f| fs::read_to_string(f).ok())
        .and_then(|t| decode(&t))
    {
        let ttl = if slow { SLOW_TTL_SECS } else { TTL_SECS };
        if k == key && (0..ttl).contains(&(now - at)) {
            return Some(g);
        }
    }
    let (g, slow) = match run(&root) {
        Some(g) => (g, false),
        None => (
            Git {
                branch: head_branch(&gitdir)?,
                dirty: None,
                ahead: 0,
                behind: 0,
            },
            true,
        ),
    };
    if let (Some(f), Some(c)) = (&file, cache_dir) {
        if fs::create_dir_all(c).is_ok() {
            let _ = fs::write(f, encode(&key, now, slow, &g));
        }
    }
    Some(g)
}

fn run(root: &Path) -> Option<Git> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
        .args(["status", "--porcelain=v2", "--branch"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    // Read on a thread so a full pipe can never deadlock the timeout.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok(buf) => {
            if !child.wait().ok()?.success() {
                return None;
            }
            Some(parse(&String::from_utf8_lossy(&buf)))
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

fn short_oid(oid: &str) -> String {
    match oid {
        "" | "(initial)" => "(initial)".to_string(),
        _ => format!("@{}", &oid[..oid.len().min(7)]),
    }
}

pub fn parse(s: &str) -> Git {
    let (mut oid, mut head) = ("", "");
    let (mut ahead, mut behind, mut dirty) = (0, 0, false);
    for line in s.lines() {
        if let Some(r) = line.strip_prefix("# branch.oid ") {
            oid = r.trim();
        } else if let Some(r) = line.strip_prefix("# branch.head ") {
            head = r.trim();
        } else if let Some(r) = line.strip_prefix("# branch.ab ") {
            for part in r.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            dirty = true;
        }
    }
    let branch = if head.is_empty() || head == "(detached)" {
        short_oid(oid)
    } else {
        head.to_string()
    };
    Git {
        branch,
        dirty: Some(dirty),
        ahead,
        behind,
    }
}

/// Branch straight from the HEAD file of a git directory.
pub fn head_branch(gitdir: &Path) -> Option<String> {
    let head = fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        return Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
    }
    (head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit())).then(|| short_oid(head))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_with_upstream() {
        let g = parse(
            "# branch.oid 4a62b81\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n",
        );
        assert_eq!(
            g,
            Git {
                branch: "main".into(),
                dirty: Some(false),
                ahead: 2,
                behind: 1
            }
        );
    }

    #[test]
    fn dirty_and_untracked() {
        let g =
            parse("# branch.oid abc\n# branch.head feat/x\n1 .M N... 100644 100644 100644 a b src/x.rs\n");
        assert_eq!(g.dirty, Some(true));
        let g = parse("# branch.head main\n? new.txt\n");
        assert_eq!(g.dirty, Some(true));
        assert_eq!((g.ahead, g.behind), (0, 0));
    }

    #[test]
    fn detached_and_initial() {
        let g = parse("# branch.oid 4a62b8151dfd86ac91ef\n# branch.head (detached)\n");
        assert_eq!(g.branch, "@4a62b81");
        let g = parse("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(g.branch, "main");
    }

    #[test]
    fn head_file_fallback() {
        let base = std::env::temp_dir().join(format!("csl-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/dev\n").unwrap();
        assert_eq!(head_branch(&git_dir(&repo).unwrap()).as_deref(), Some("dev"));
        fs::create_dir_all(repo.join("sub/deep")).unwrap();
        assert_eq!(find_root(&repo.join("sub/deep")).as_deref(), Some(repo.as_path()));

        let wt = base.join("wt");
        let wt_git = base.join("gitdirs/wt");
        fs::create_dir_all(&wt).unwrap();
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        fs::write(wt_git.join("HEAD"), "4a62b8151dfd86ac91ef401c04ed1c3556892e50\n").unwrap();
        assert_eq!(git_dir(&wt).as_deref(), Some(wt_git.as_path()));
        assert_eq!(head_branch(&wt_git).as_deref(), Some("@4a62b81"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn cache_round_trip() {
        let g = Git {
            branch: "feat/x".into(),
            dirty: None,
            ahead: 3,
            behind: 0,
        };
        let text = encode("1:2:3:0", 99, true, &g);
        assert_eq!(decode(&text), Some(("1:2:3:0".to_string(), 99, true, g)));
        assert_eq!(decode("junk"), None);
        assert_eq!(decode("k\n1\n\n0\n0\n0\n0\n"), None, "empty branch rejected");
    }

    #[test]
    fn cached_status_in_real_repo() {
        let base = std::env::temp_dir().join(format!("csl-gitrun-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("r");
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if !git(&["init", "-q", "-b", "trunk"]) {
            return; // no git on this machine
        }
        let cache = base.join("cache");
        let g = status(&repo, Some(&cache), 1000).unwrap();
        assert_eq!(
            g,
            Git {
                branch: "trunk".into(),
                dirty: Some(false),
                ahead: 0,
                behind: 0
            }
        );
        fs::write(repo.join("new.txt"), "x").unwrap();
        // Within the TTL and with unchanged metadata the cached answer stands.
        assert_eq!(status(&repo, Some(&cache), 1002).unwrap().dirty, Some(false));
        // After the TTL git runs again and sees the untracked file.
        assert_eq!(
            status(&repo, Some(&cache), 1000 + TTL_SECS).unwrap().dirty,
            Some(true)
        );
        // Staging rewrites the index, which invalidates the cache at once.
        assert!(git(&["add", "new.txt"]));
        fs::remove_file(repo.join("new.txt")).unwrap();
        let g = status(&repo, Some(&cache), 1000 + TTL_SECS + 1).unwrap();
        assert_eq!(g.dirty, Some(true));
        let _ = fs::remove_dir_all(&base);
    }
}
