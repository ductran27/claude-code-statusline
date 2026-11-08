//! Build the status line from Claude Code's stdin JSON.
//!
//! Layout: `project│branch ✓│tokens/ctx│5h N% (reset)/7d N% (reset)/F N%│Model (ctx|EFFORT|ULTRA|FAST)│style`

use crate::color::{
    by_pct, BOLD, DIM, GRAY, GREEN, MAUVE, ORANGE, RATE_THRESH, RED, RESET, ROSE, SEP, SLATE, TOKEN_THRESH,
    WHITE_DIM, YELLOW,
};
use crate::{git, scoped, time, ultra};
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct Ctx {
    pub now: i64,
    pub home: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub git: bool,
    pub scoped: bool,
    pub ultra: bool,
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn num_at(v: &Value, ptr: &str) -> Option<f64> {
    v.pointer(ptr).and_then(Value::as_f64).filter(|f| f.is_finite())
}

/// Drop a trailing "(1M context)" style note; the context size has its own slot.
fn strip_context_note(s: &str) -> &str {
    match s.rfind(" (") {
        Some(i) if s.ends_with(')') && s[i..].to_ascii_lowercase().contains("context") => s[..i].trim_end(),
        _ => s,
    }
}

const FAMILIES: [&str; 5] = ["fable", "mythos", "opus", "sonnet", "haiku"];

/// `claude-opus-5-5` → `Opus 5.5`, `claude-3-7-sonnet-20250219` → `Sonnet 3.7`,
/// also for Bedrock (`us.anthropic.…-v1:0`), Vertex (`…@date`) and `[1m]` ids.
pub fn label_from_id(id: &str) -> String {
    let id = id.split('[').next().unwrap_or(id);
    let id = id.split('@').next().unwrap_or(id);
    let id = id.rsplit('.').next().unwrap_or(id);
    let id = id.split(':').next().unwrap_or(id).trim();
    let toks: Vec<&str> = id.split('-').collect();
    let Some(fam) = toks
        .iter()
        .find_map(|t| FAMILIES.iter().find(|f| t.eq_ignore_ascii_case(f)))
    else {
        return id.to_string();
    };
    let mut ver: Vec<&str> = Vec::new();
    for t in &toks {
        if (1..=2).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit()) {
            ver.push(t);
            if ver.len() == 2 {
                break;
            }
        } else if !ver.is_empty() {
            break;
        }
    }
    let mut label = fam[..1].to_ascii_uppercase() + &fam[1..];
    if !ver.is_empty() {
        label.push(' ');
        label.push_str(&ver.join("."));
    }
    label
}

/// Claude Code's own display name ("Opus 5.5", "Fable 5.1") when it has
/// one, else a label parsed from the model id.
pub fn model_label(v: &Value) -> String {
    if let Some(d) = str_at(v, "/model/display_name") {
        let d = strip_context_note(d.strip_prefix("Claude ").unwrap_or(d));
        if !d.is_empty() && d != "Claude" && !d.eq_ignore_ascii_case("unknown model") {
            return d.to_string();
        }
    }
    str_at(v, "/model/id").map(label_from_id).unwrap_or_default()
}

/// `.effort.level` is the tier Claude Code resolves for the next request
/// (low, medium, high, xhigh, max), already capped to what the model
/// supports. It is absent when the model takes no effort parameter, and
/// then nothing is shown: a settings value would not be what runs.
pub fn effort(v: &Value) -> Option<(String, &'static str)> {
    let lvl = str_at(v, "/effort/level")?.to_ascii_lowercase();
    let color = match lvl.as_str() {
        "low" => GRAY,
        "medium" => WHITE_DIM,
        "high" => GREEN,
        "xhigh" => YELLOW,
        "max" => RED,
        _ => "",
    };
    Some((lvl.to_ascii_uppercase(), color))
}

pub fn ctx_label(n: u64) -> String {
    if n >= 1_000_000 {
        if n % 1_000_000 == 0 {
            format!("{}M", n / 1_000_000)
        } else {
            format!("{:.1}M", n as f64 / 1e6)
        }
    } else if n >= 1000 {
        format!("{}K", n / 1000)
    } else {
        n.to_string()
    }
}

pub fn tokens_label(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

/// Context in use, counted the way Claude Code counts it for
/// `used_percentage`: input plus both cache fields of the last response.
fn context(v: &Value) -> Option<(u64, u64, i64)> {
    let size = num_at(v, "/context_window/context_window_size")? as u64;
    if size == 0 {
        return None;
    }
    let field = |k: &str| {
        num_at(v, &format!("/context_window/current_usage/{k}"))
            .unwrap_or(0.0)
            .max(0.0) as u64
    };
    let used =
        field("input_tokens") + field("cache_creation_input_tokens") + field("cache_read_input_tokens");
    let pct = num_at(v, "/context_window/used_percentage")
        .map_or_else(|| (used as f64 * 100.0 / size as f64).round(), f64::round) as i64;
    Some((used, size, pct.clamp(0, 100)))
}

fn rate(v: &Value, key: &str) -> Option<(i64, f64, Option<i64>)> {
    let pct = num_at(v, &format!("/rate_limits/{key}/used_percentage"))?;
    let reset = num_at(v, &format!("/rate_limits/{key}/resets_at"))
        .map(|r| r.round() as i64)
        .filter(|&r| r > 0);
    Some((pct.round().max(0.0) as i64, pct, reset))
}

fn rate_part(out: &mut String, label: &str, pct: i64, reset: Option<i64>, now: i64) {
    let c = by_pct(pct, RATE_THRESH);
    let _ = write!(out, "{BOLD}{WHITE_DIM}{label}{RESET}{c} {pct}%{RESET}");
    if let Some(r) = reset {
        let _ = write!(out, " {DIM}({}){RESET}", time::reset_label(r, now));
    }
}

fn git_segment(g: &git::Git) -> String {
    let mut s = format!("{RESET}{}", g.branch);
    match g.dirty {
        Some(true) => {
            let _ = write!(s, " {ORANGE}✗{RESET}");
        }
        Some(false) => {
            let _ = write!(s, " {GREEN}✓{RESET}");
        }
        None => {}
    }
    if g.ahead > 0 {
        let _ = write!(s, " {SLATE}↑{}{RESET}", g.ahead);
    }
    if g.behind > 0 {
        let _ = write!(s, " {MAUVE}↓{}{RESET}", g.behind);
    }
    s
}

pub fn render(v: &Value, cx: &Ctx) -> String {
    let mut segs: Vec<String> = Vec::with_capacity(6);

    let dir = str_at(v, "/workspace/project_dir")
        .or_else(|| str_at(v, "/cwd"))
        .or_else(|| str_at(v, "/workspace/current_dir"));
    if let Some(name) = dir
        .and_then(|d| Path::new(d).file_name())
        .and_then(|n| n.to_str())
    {
        segs.push(format!("{YELLOW}{name}{RESET}"));
    }
    if cx.git {
        if let Some(g) = dir.and_then(|d| git::status(Path::new(d), cx.cache_dir.as_deref(), cx.now)) {
            segs.push(git_segment(&g));
        }
    }

    let ctx = context(v);
    if let Some((used, size, pct)) = ctx {
        let c = by_pct(pct, TOKEN_THRESH);
        segs.push(format!("{c}{}/{}{RESET}", tokens_label(used), ctx_label(size)));
    }

    let five = rate(v, "five_hour");
    let seven = rate(v, "seven_day");
    let mut rates = String::new();
    let add = |rates: &mut String, label: &str, pct: i64, reset: Option<i64>| {
        if !rates.is_empty() {
            let _ = write!(rates, "{DIM}/{RESET}");
        }
        rate_part(rates, label, pct, reset, cx.now);
    };
    if let Some((pct, _, reset)) = five {
        add(&mut rates, "5h", pct, reset);
    }
    if let Some((pct, _, reset)) = seven {
        add(&mut rates, "7d", pct, reset);
    }
    if cx.scoped {
        if let (Some(home), Some(cache)) = (&cx.home, &cx.cache_dir) {
            let seven_key = seven.and_then(|(_, raw, reset)| reset.map(|r| (r, raw)));
            for b in scoped::buckets(home, cache, cx.now, seven_key) {
                let label: String = b.name.chars().take(1).flat_map(char::to_uppercase).collect();
                // The buckets normally reset with the 7d window; repeat the
                // time only when it is a different moment.
                let seven_reset = seven.and_then(|s| s.2);
                let reset = b
                    .resets_at
                    .filter(|r| seven_reset.map_or(true, |s| (r - s).abs() > 3600));
                add(&mut rates, &label, b.pct, reset);
            }
        }
    }
    if !rates.is_empty() {
        segs.push(rates);
    }

    let mut inner: Vec<String> = Vec::with_capacity(3);
    if let Some((_, size, _)) = ctx {
        inner.push(ctx_label(size));
    }
    if let Some((lvl, c)) = effort(v) {
        inner.push(format!("{BOLD}{c}{lvl}{RESET}{DIM}"));
        // Only models with an effort setting can run ultracode.
        let mark = match (cx.ultra, str_at(v, "/transcript_path"), cx.cache_dir.as_deref()) {
            (true, Some(t), Some(cache)) => ultra::mark(
                Path::new(t),
                cache,
                str_at(v, "/model/id").unwrap_or(""),
                str_at(v, "/prompt_id"),
            ),
            _ => None,
        };
        match mark {
            Some(ultra::Mark::Session) => inner.push(format!("{BOLD}{MAUVE}ULTRA{RESET}{DIM}")),
            Some(ultra::Mark::Turn) => inner.push(format!("{MAUVE}ultra{RESET}{DIM}")),
            None => {}
        }
    }
    if v.pointer("/fast_mode").and_then(Value::as_bool) == Some(true) {
        inner.push(format!("{ORANGE}FAST{RESET}{DIM}"));
    }
    let label = model_label(v);
    let mut model = String::new();
    if !label.is_empty() {
        let _ = write!(model, "{SLATE}{label}{RESET}");
    }
    if !inner.is_empty() {
        let space = if label.is_empty() { "" } else { " " };
        let _ = write!(model, "{space}{DIM}({}){RESET}", inner.join("|"));
    }
    if !model.is_empty() {
        segs.push(model);
    }

    if let Some(style) = str_at(v, "/output_style/name").filter(|s| !s.eq_ignore_ascii_case("default")) {
        segs.push(format!("{ROSE}{style}{RESET}"));
    }

    segs.join(SEP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for d in it.by_ref() {
                    if d == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn cx() -> Ctx {
        Ctx {
            now: 1_794_160_000,
            home: None,
            cache_dir: None,
            git: false,
            scoped: false,
            ultra: false,
        }
    }

    #[test]
    fn ids_to_labels() {
        for (id, want) in [
            ("claude-opus-5-5", "Opus 5.5"),
            ("claude-fable-5-1[1m]", "Fable 5.1"),
            ("claude-fable-5", "Fable 5"),
            ("claude-opus-4-1-20250805", "Opus 4.1"),
            ("claude-sonnet-4-20250514", "Sonnet 4"),
            ("claude-3-7-sonnet-20250219", "Sonnet 3.7"),
            ("us.anthropic.claude-opus-4-5-20251101-v1:0", "Opus 4.5"),
            ("claude-opus-4-1@20250805", "Opus 4.1"),
            ("claude-haiku-4-5", "Haiku 4.5"),
            ("claude-mythos-5", "Mythos 5"),
            ("some-custom-model", "some-custom-model"),
        ] {
            assert_eq!(label_from_id(id), want, "{id}");
        }
    }

    #[test]
    fn display_name_preferred_and_cleaned() {
        let v = json!({"model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"}});
        assert_eq!(model_label(&v), "Opus 5.5");
        let v =
            json!({"model": {"id": "claude-opus-4-6[1m]", "display_name": "Claude Opus 4.6 (1M context)"}});
        assert_eq!(model_label(&v), "Opus 4.6");
        let v = json!({"model": {"id": "claude-fable-5-1", "display_name": "unknown model"}});
        assert_eq!(model_label(&v), "Fable 5.1");
        let v = json!({"model": {"id": "claude-fable-5-1"}});
        assert_eq!(model_label(&v), "Fable 5.1");
    }

    #[test]
    fn effort_is_shown_exactly() {
        for (lvl, want) in [
            ("low", "LOW"),
            ("medium", "MEDIUM"),
            ("high", "HIGH"),
            ("xhigh", "XHIGH"),
            ("max", "MAX"),
        ] {
            assert_eq!(effort(&json!({"effort": {"level": lvl}})).unwrap().0, want);
        }
        assert_eq!(effort(&json!({"effort": {"level": "max"}})).unwrap().1, RED);
        assert_eq!(effort(&json!({"effort": {"level": "xhigh"}})).unwrap().1, YELLOW);
        assert!(effort(&json!({})).is_none(), "no effort parameter, no tier");
        assert!(effort(&json!({"effort": null})).is_none());
        assert!(effort(&json!({"effort": {"level": ""}})).is_none());
        assert_eq!(
            effort(&json!({"effort": {"level": "turbo"}})).unwrap(),
            ("TURBO".to_string(), "")
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(ctx_label(1_000_000), "1M");
        assert_eq!(ctx_label(200_000), "200K");
        assert_eq!(ctx_label(1_500_000), "1.5M");
        assert_eq!(tokens_label(92_043), "92k");
        assert_eq!(tokens_label(999), "999");
        assert_eq!(tokens_label(1_234_567), "1.2m");
    }

    #[test]
    fn full_line() {
        let v = json!({
            "workspace": {"project_dir": "/Users/x/code/proj"},
            "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
            "effort": {"level": "max"}, "fast_mode": true,
            "output_style": {"name": "Concise"},
            "context_window": {"context_window_size": 1_000_000, "used_percentage": 9,
                "current_usage": {"input_tokens": 2, "output_tokens": 1567,
                    "cache_creation_input_tokens": 1715, "cache_read_input_tokens": 90326}},
            "rate_limits": {"five_hour": {"used_percentage": 28.000000000000004, "resets_at": 1_794_160_000 + 9360},
                            "seven_day": {"used_percentage": 70.6, "resets_at": 1_794_160_000 + 30}}
        });
        assert_eq!(
            plain(&render(&v, &cx())),
            "proj│92k/1M│5h 28% (2h36m)/7d 71% (1m)│Opus 5.5 (1M|MAX|FAST)│Concise"
        );
    }

    #[test]
    fn sparse_payload() {
        let v = json!({"model": {"id": "claude-haiku-4-5", "display_name": "Haiku 4.5"},
                       "output_style": {"name": "default"},
                       "context_window": {"context_window_size": 200000, "used_percentage": null, "current_usage": null}});
        assert_eq!(plain(&render(&v, &cx())), "0/200K│Haiku 4.5 (200K)");
        assert_eq!(plain(&render(&json!({}), &cx())), "");
    }
}
