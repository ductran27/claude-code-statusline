//! End-to-end: run the binary on payloads with a throwaway home directory.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const NOW: i64 = 1_794_160_000; // 2026-11-08T17:46:40Z
const SEVEN_RESET: i64 = 1_794_729_600; // 2026-11-15T08:00:00Z

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("csl-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Home(p)
    }
    fn write(&self, rel: &str, body: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(home: &Path, payload: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-statusline"))
        .env_clear()
        // Windows needs SystemRoot to start a process normally.
        .envs(std::env::vars().filter(|(k, _)| k.eq_ignore_ascii_case("SystemRoot")))
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", home.join("cache"))
        .env("STATUSLINE_NOW", NOW.to_string())
        .env("STATUSLINE_GIT", "off")
        .env("TZ", "UTC0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "exit {:?}", out.status);
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
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

fn payload(effort: &str, rate_limits: &str) -> String {
    format!(
        r#"{{
  "session_id": "s", "cwd": "/w/proj",
  "model": {{"id": "claude-opus-5-5", "display_name": "Opus 5.5"}},
  "workspace": {{"current_dir": "/w/proj", "project_dir": "/w/proj"}},
  "output_style": {{"name": "Concise"}},
  "context_window": {{"context_window_size": 1000000, "used_percentage": 9,
    "current_usage": {{"input_tokens": 2, "output_tokens": 1567,
      "cache_creation_input_tokens": 1715, "cache_read_input_tokens": 90326}}}},
  {effort}
  "fast_mode": false,
  "thinking": {{"enabled": true}}{rate_limits}
}}"#
    )
}

fn seven(pct: f64, reset: i64) -> String {
    format!(r#", "rate_limits": {{"seven_day": {{"used_percentage": {pct}, "resets_at": {reset}}}}}"#)
}

fn usage_json(rows: &[(&str, &str, f64, f64, &str)]) -> String {
    let accounts: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, (email, reset, pct, fable, fable_reset))| {
            format!(
                r#""{i}": {{"email": "{email}", "organizationUuid": "org-{email}",
                  "fetchedAt": {fetched}, "lastGood": {{
                  "seven_day": {{"pct": {pct}, "resets_at": "{reset}"}},
                  "scoped": [{{"name": "Fable", "pct": {fable}, "resets_at": "{fable_reset}"}}]}}}}"#,
                fetched = NOW - 60
            )
        })
        .collect();
    format!(
        r#"{{"schemaVersion": 2, "accounts": {{{}}}}}"#,
        accounts.join(",")
    )
}

const CSWAP: &str = ".claude-swap-backup/cache/usage.json";

#[test]
fn full_line_with_session_bucket() {
    let h = Home::new("full");
    h.write(
        CSWAP,
        &usage_json(&[
            (
                "other",
                "2026-11-09T05:59:59.7+00:00",
                100.0,
                38.0,
                "2026-11-09T05:59:59.7+00:00",
            ),
            (
                "me",
                "2026-11-15T08:00:00.505763+00:00",
                71.0,
                12.4,
                "2026-11-15T08:00:00+00:00",
            ),
        ]),
    );
    let out = run(
        &h.0,
        &payload(r#""effort": {"level": "max"},"#, &seven(70.6, SEVEN_RESET)),
    );
    assert_eq!(
        plain(&out),
        "proj│92k/1M│7d 71% (Sun 8AM)/F 12%│Opus 5.5 (1M|MAX)│Concise"
    );
    assert!(
        out.contains("\x1b[1m\x1b[38;5;167mMAX"),
        "MAX is bold brick: {out:?}"
    );
}

#[test]
fn bucket_with_its_own_reset_shows_it() {
    let h = Home::new("ownreset");
    h.write(
        CSWAP,
        &usage_json(&[(
            "me",
            "2026-11-15T08:00:00+00:00",
            71.0,
            50.0,
            "2026-11-08T20:00:00+00:00",
        )]),
    );
    let out = plain(&run(
        &h.0,
        &payload(r#""effort": {"level": "xhigh"},"#, &seven(71.0, SEVEN_RESET)),
    ));
    assert!(out.contains("/F 50% (2h14m)│"), "{out}");
    assert!(out.ends_with("Opus 5.5 (1M|XHIGH)│Concise"), "{out}");
}

#[test]
fn bucket_of_another_window_is_hidden() {
    let h = Home::new("mismatch");
    h.write(
        CSWAP,
        &usage_json(&[(
            "other",
            "2026-11-12T08:00:00+00:00",
            10.0,
            90.0,
            "2026-11-12T08:00:00+00:00",
        )]),
    );
    h.write(
        ".claude.json",
        r#"{"oauthAccount": {"emailAddress": "other", "organizationUuid": "org-other"}}"#,
    );
    let out = plain(&run(
        &h.0,
        &payload(r#""effort": {"level": "high"},"#, &seven(71.0, SEVEN_RESET)),
    ));
    assert!(!out.contains("F "), "{out}");
}

#[test]
fn without_rate_limits_the_logged_in_account_is_used() {
    let h = Home::new("identity");
    h.write(
        CSWAP,
        &usage_json(&[
            (
                "a",
                "2026-11-12T08:00:00+00:00",
                10.0,
                90.0,
                "2026-11-12T08:00:00+00:00",
            ),
            (
                "b",
                "2026-11-13T08:00:00+00:00",
                20.0,
                7.0,
                "2026-11-13T08:00:00+00:00",
            ),
        ]),
    );
    h.write(
        ".claude.json",
        r#"{"x": 1, "oauthAccount": {"emailAddress": "b", "organizationUuid": "org-b"}}"#,
    );
    let out = plain(&run(&h.0, &payload(r#""effort": {"level": "low"},"#, "")));
    assert_eq!(out, "proj│92k/1M│F 7% (Fri 8AM)│Opus 5.5 (1M|LOW)│Concise");
}

#[test]
fn bucket_past_its_reset_is_dropped() {
    let h = Home::new("expired");
    h.write(
        CSWAP,
        &usage_json(&[(
            "me",
            "2026-11-08T08:00:00+00:00",
            99.0,
            99.0,
            "2026-11-08T08:00:00+00:00",
        )]),
    );
    h.write(
        ".claude.json",
        r#"{"oauthAccount": {"emailAddress": "me", "organizationUuid": "org-me"}}"#,
    );
    let out = plain(&run(&h.0, &payload("", "")));
    assert_eq!(out, "proj│92k/1M│Opus 5.5 (1M)│Concise");
}

#[test]
fn no_effort_field_means_no_tier_even_with_settings() {
    let h = Home::new("noeffort");
    h.write(".claude/settings.json", r#"{"effortLevel": "xhigh"}"#);
    let out = plain(&run(&h.0, &payload("", &seven(5.0, SEVEN_RESET))));
    assert_eq!(out, "proj│92k/1M│7d 5% (Sun 8AM)│Opus 5.5 (1M)│Concise");
}

#[test]
fn bad_input_prints_nothing() {
    let h = Home::new("bad");
    assert_eq!(run(&h.0, "not json"), "");
    assert_eq!(run(&h.0, ""), "");
}

#[test]
fn every_effort_tier_round_trips() {
    let h = Home::new("tiers");
    for (lvl, want) in [
        ("low", "LOW"),
        ("medium", "MEDIUM"),
        ("high", "HIGH"),
        ("xhigh", "XHIGH"),
        ("max", "MAX"),
    ] {
        let out = plain(&run(
            &h.0,
            &payload(&format!(r#""effort": {{"level": "{lvl}"}},"#), ""),
        ));
        assert!(
            out.ends_with(&format!("Opus 5.5 (1M|{want})│Concise")),
            "{lvl}: {out}"
        );
    }
}
