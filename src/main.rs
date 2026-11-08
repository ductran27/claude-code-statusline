//! claude-statusline: a fast, accurate status line for Claude Code.
//!
//! Claude Code pipes a JSON description of the session to the command on
//! every render; one line goes back on stdout. Environment switches:
//!
//! - `STATUSLINE_GIT=off`      skip the git segment
//! - `STATUSLINE_SCOPED=off`   skip the per-model weekly buckets (cswap)
//! - `STATUSLINE_ULTRA=off`    skip the ultracode marker and the transcript read
//! - `SMARTBAR_CSWAP=<path>`   cswap binary used for background refreshes
//! - `STATUSLINE_CSWAP_USAGE=<path>`  cswap `usage.json` to read

mod color;
mod git;
mod render;
mod scoped;
mod time;
mod ultra;

use std::env;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn home() -> Option<PathBuf> {
    let var = |k| env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("USERPROFILE").or_else(|| var("HOME"))
    } else {
        var("HOME")
    }
}

fn cache_dir(home: Option<&PathBuf>) -> Option<PathBuf> {
    let var = |k| env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
    let base = if cfg!(windows) {
        var("LOCALAPPDATA").or_else(|| home.map(|h| h.join(".cache")))
    } else {
        var("XDG_CACHE_HOME").or_else(|| home.map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("claude-statusline"))
}

fn on(key: &str) -> bool {
    !env::var(key).is_ok_and(|v| v.eq_ignore_ascii_case("off") || v == "0")
}

fn main() {
    let mut buf = Vec::with_capacity(8192);
    if std::io::stdin().lock().read_to_end(&mut buf).is_err() {
        return;
    }
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&buf) else {
        return;
    };
    let now = env::var("STATUSLINE_NOW")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64)
        });
    let home = home();
    let cx = render::Ctx {
        now,
        cache_dir: cache_dir(home.as_ref()),
        home,
        git: on("STATUSLINE_GIT"),
        scoped: on("STATUSLINE_SCOPED"),
        ultra: on("STATUSLINE_ULTRA"),
    };
    let line = render::render(&payload, &cx);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}
