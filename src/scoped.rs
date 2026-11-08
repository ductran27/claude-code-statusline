//! Per-model weekly buckets (the "Fable" row of /usage) from claude-swap.
//!
//! The status line stdin carries only the account-wide 5h and 7d windows.
//! claude-swap keeps the last good /usage reading of every managed account in
//! `cache/usage.json`; that file is read directly, so a render never starts
//! Python and never touches the network. The row that belongs to this
//! session is the one whose 7d reset equals the 7d reset on stdin, which
//! stays right even when cswap's active account is not the session's own.

use crate::time::parse_iso;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// Rows fetched longer ago than this ask cswap for a fresh reading.
const STALE_SECS: f64 = 600.0;
/// At most one background cswap refresh per this many seconds.
const REFRESH_EVERY: Duration = Duration::from_secs(300);
/// Window resets from stdin (whole seconds) and cswap (fractional) agree
/// to within a second; anything this far apart is another window.
const MATCH_SECS: i64 = 120;

#[derive(Debug, PartialEq)]
pub struct Bucket {
    pub name: String,
    pub pct: i64,
    pub resets_at: Option<i64>,
}

#[derive(Debug)]
struct Row {
    email: String,
    org: String,
    seven_reset: Option<i64>,
    seven_pct: Option<f64>,
    scoped: Vec<(String, f64, Option<i64>)>,
    fetched_at: Option<f64>,
}

fn epoch(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => parse_iso(s).map(|f| f.round() as i64),
        Value::Number(n) => n.as_f64().map(|f| f.round() as i64),
        _ => None,
    }
}

fn usage_paths(home: &Path) -> Vec<PathBuf> {
    if let Some(p) = env::var_os("STATUSLINE_CSWAP_USAGE") {
        return vec![PathBuf::from(p)];
    }
    let mut v = Vec::new();
    if cfg!(target_os = "linux") {
        if let Some(x) = env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
            if x.is_absolute() {
                v.push(x.join("claude-swap/cache/usage.json"));
            }
        }
        v.push(home.join(".local/share/claude-swap/cache/usage.json"));
    }
    v.push(home.join(".claude-swap-backup").join("cache").join("usage.json"));
    v
}

fn parse_rows(doc: &Value) -> Vec<Row> {
    let Some(accounts) = doc.get("accounts").and_then(Value::as_object) else {
        return Vec::new();
    };
    accounts
        .values()
        .filter_map(|r| {
            let good = r.get("lastGood")?;
            let seven = good.get("seven_day");
            let scoped = good
                .get("scoped")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|b| {
                            let name = b.get("name")?.as_str()?.trim();
                            let pct = b.get("pct")?.as_f64()?;
                            (!name.is_empty())
                                .then(|| (name.to_string(), pct, b.get("resets_at").and_then(epoch)))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Row {
                email: r.get("email")?.as_str()?.to_string(),
                org: r
                    .get("organizationUuid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                seven_reset: seven.and_then(|s| s.get("resets_at")).and_then(epoch),
                seven_pct: seven.and_then(|s| s.get("pct")).and_then(Value::as_f64),
                scoped,
                fetched_at: r.get("fetchedAt").and_then(Value::as_f64),
            })
        })
        .collect()
}

/// The logged-in account (`oauthAccount`) of Claude Code's global config.
/// The file is large; only the one object is parsed.
fn identity(home: &Path) -> Option<(String, String)> {
    let base = env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let legacy = base
        .clone()
        .unwrap_or_else(|| home.join(".claude"))
        .join(".config.json");
    let path = if legacy.is_file() {
        legacy
    } else {
        base.unwrap_or_else(|| home.to_path_buf()).join(".claude.json")
    };
    let bytes = fs::read(path).ok()?;
    let key = b"\"oauthAccount\"";
    let at = bytes.windows(key.len()).position(|w| w == key)? + key.len();
    let rest = &bytes[at..];
    let colon = rest.iter().position(|c| !c.is_ascii_whitespace())?;
    if rest[colon] != b':' {
        return None;
    }
    let acct: Value = serde_json::Deserializer::from_slice(&rest[colon + 1..])
        .into_iter::<Value>()
        .next()?
        .ok()?;
    Some((
        acct.get("emailAddress")?.as_str()?.to_string(),
        acct.get("organizationUuid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ))
}

fn is_identity(r: &Row, id: Option<&(String, String)>) -> bool {
    id.is_some_and(|(e, o)| &r.email == e && &r.org == o)
}

/// The logged-in account's row. A re-added account can leave an old slot
/// with the same identity behind, so the most recently fetched one wins.
fn identity_row<'a>(
    rows: impl IntoIterator<Item = &'a Row>,
    id: Option<&(String, String)>,
) -> Option<&'a Row> {
    rows.into_iter().filter(|r| is_identity(r, id)).max_by(|a, b| {
        a.fetched_at
            .unwrap_or(0.0)
            .total_cmp(&b.fetched_at.unwrap_or(0.0))
    })
}

/// Choose this session's row. `seven` is stdin's 7d (reset epoch, pct).
fn pick(
    rows: &[Row],
    seven: Option<(i64, f64)>,
    id: impl FnOnce() -> Option<(String, String)>,
) -> (Option<&Row>, Option<&Row>) {
    let Some((reset, pct)) = seven else {
        let id = id();
        let row = identity_row(rows, id.as_ref());
        return (row, row);
    };
    let cands: Vec<&Row> = rows
        .iter()
        .filter(|r| r.seven_reset.is_some_and(|s| (s - reset).abs() <= MATCH_SECS))
        .collect();
    if cands.len() == 1 {
        return (Some(cands[0]), None);
    }
    let id = id();
    if cands.is_empty() {
        // No managed account is in this session's window: show nothing,
        // but report cswap's own row so a stale reading gets refreshed.
        return (None, identity_row(rows, id.as_ref()));
    }
    let chosen = identity_row(cands.iter().copied(), id.as_ref()).or_else(|| {
        cands.iter().copied().min_by(|a, b| {
            let da = (a.seven_pct.unwrap_or(f64::MAX) - pct).abs();
            let db = (b.seven_pct.unwrap_or(f64::MAX) - pct).abs();
            da.total_cmp(&db)
        })
    });
    (chosen, None)
}

fn cswap_bin(home: &Path) -> Option<PathBuf> {
    if let Some(p) = env::var_os("SMARTBAR_CSWAP").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["cswap.exe", "cswap.cmd", "cswap.bat"]
    } else {
        &["cswap"]
    };
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            for n in names {
                let c = dir.join(n);
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    let local = home.join(".local").join("bin").join(names[0]);
    local.is_file().then_some(local)
}

/// Ask cswap to refresh the active account in the background. The child
/// gets its own process group, so Claude Code cancelling a render (which
/// kills the status line's group) cannot kill it halfway.
fn refresh(home: &Path, cache_dir: &Path) {
    let stamp = cache_dir.join("cswap-refresh.stamp");
    let recent = fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < REFRESH_EVERY);
    if recent {
        return;
    }
    let Some(bin) = cswap_bin(home) else { return };
    if fs::create_dir_all(cache_dir).is_err() || fs::write(&stamp, b"").is_err() {
        return;
    }
    let mut cmd = Command::new(bin);
    cmd.args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let _ = cmd.spawn();
}

pub fn buckets(home: &Path, cache_dir: &Path, now: i64, seven: Option<(i64, f64)>) -> Vec<Bucket> {
    let doc = usage_paths(home)
        .iter()
        .find_map(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let Some(doc) = doc else {
        refresh(home, cache_dir);
        return Vec::new();
    };
    let rows = parse_rows(&doc);
    let (chosen, cswap_row) = pick(&rows, seven, || identity(home));
    let stale = |r: &Row| {
        r.fetched_at.map_or(true, |f| now as f64 - f > STALE_SECS) || r.seven_reset.is_some_and(|s| s <= now)
    };
    // With neither row, refresh only while the file holds no rows at all;
    // otherwise this session's account is simply not managed by cswap.
    if chosen.or(cswap_row).map_or(rows.is_empty(), stale) {
        refresh(home, cache_dir);
    }
    let Some(row) = chosen else { return Vec::new() };
    row.scoped
        .iter()
        .filter(|(_, _, reset)| reset.map_or(true, |r| r > now))
        .map(|(name, pct, reset)| Bucket {
            name: name.clone(),
            pct: pct.round().max(0.0) as i64,
            resets_at: *reset,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(email: &str, reset: &str, pct: f64, fable: f64) -> Value {
        json!({
            "email": email, "organizationUuid": format!("org-{email}"),
            "fetchedAt": 1_794_160_050.5,
            "lastGood": {
                "seven_day": {"pct": pct, "resets_at": reset},
                "scoped": [{"name": "Fable", "pct": fable, "resets_at": reset}]
            }
        })
    }

    fn doc() -> Vec<Row> {
        parse_rows(&json!({"schemaVersion": 2, "accounts": {
            "3": row("a", "2026-11-09T05:59:59.739690+00:00", 100.0, 38.0),
            "7": row("b", "2026-11-15T08:00:00.505763+00:00", 71.0, 4.4),
            "9": row("c", "2026-11-15T16:00:00+00:00", 0.0, 0.0),
            "12": row("d", "2026-11-15T08:00:00+00:00", 12.0, 9.0),
            "x": {"email": "broken"}
        }}))
    }

    const NOV15_08: i64 = 1_794_729_600;

    #[test]
    fn parses_rows() {
        let rows = doc();
        assert_eq!(rows.len(), 4, "row without lastGood is skipped");
        let a = rows.iter().find(|r| r.email == "a").unwrap();
        assert_eq!(a.seven_reset, Some(1_794_203_999 + 1)); // 05:59:59.74 rounds to 06:00:00
        assert_eq!(a.scoped[0].0, "Fable");
    }

    #[test]
    fn unique_reset_match_wins_without_identity() {
        let rows = doc();
        let (r, _) = pick(&rows, Some((1_794_204_000, 100.0)), || {
            panic!("identity not needed")
        });
        assert_eq!(r.unwrap().email, "a");
    }

    #[test]
    fn tie_prefers_identity_then_closest_pct() {
        let rows = doc();
        let (r, _) = pick(&rows, Some((NOV15_08, 71.0)), || {
            Some(("d".into(), "org-d".into()))
        });
        assert_eq!(r.unwrap().email, "d");
        let (r, _) = pick(&rows, Some((NOV15_08, 70.0)), || None);
        assert_eq!(r.unwrap().email, "b");
        let (r, _) = pick(&rows, Some((NOV15_08, 13.0)), || None);
        assert_eq!(r.unwrap().email, "d");
    }

    #[test]
    fn no_window_match_shows_nothing() {
        let rows = doc();
        let (r, cs) = pick(&rows, Some((NOV15_08 + 86_400, 5.0)), || {
            Some(("c".into(), "org-c".into()))
        });
        assert!(r.is_none());
        assert_eq!(cs.unwrap().email, "c");
    }

    #[test]
    fn without_stdin_window_identity_is_used() {
        let rows = doc();
        let (r, _) = pick(&rows, None, || Some(("c".into(), "org-c".into())));
        assert_eq!(r.unwrap().email, "c");
        let (r, _) = pick(&rows, None, || None);
        assert!(r.is_none());
    }

    #[test]
    fn duplicate_identity_prefers_freshest_slot() {
        let mut old = row("c", "2026-07-26T17:00:00+00:00", 79.0, 100.0);
        old["fetchedAt"] = json!(1_784_736_207.1);
        let mut new = row("c", "2026-11-15T17:00:00+00:00", 0.0, 0.0);
        new["fetchedAt"] = json!(1_794_161_191.9);
        let rows = parse_rows(&json!({"accounts": {"1": old, "4": new}}));
        let id = || Some(("c".to_string(), "org-c".to_string()));
        let (r, _) = pick(&rows, None, id);
        assert_eq!(r.unwrap().seven_pct, Some(0.0));
        let (r, cs) = pick(&rows, Some((NOV15_08 + 86_400 * 3, 5.0)), id);
        assert!(r.is_none());
        assert_eq!(cs.unwrap().seven_pct, Some(0.0));
    }

    #[test]
    fn identity_reads_only_oauth_account() {
        let dir = std::env::temp_dir().join(format!("csl-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(".claude.json"),
            r#"{"numStartups": 3, "oauthAccount" : {"emailAddress": "e@x", "organizationUuid": "o-1", "nested": {"k": [1,2]}}, "zzz": true}"#,
        )
        .unwrap();
        assert_eq!(identity(&dir), Some(("e@x".into(), "o-1".into())));
        let _ = fs::remove_dir_all(&dir);
    }
}
