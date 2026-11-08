//! Ultracode marker, read from the session transcript.
//!
//! The status line JSON has no ultracode field. Claude Code records the
//! state in the transcript: an `ultra_effort_enter` or `ultra_effort_exit`
//! attachment when a prompt starts with ultracode turned on or off, the
//! output of `/effort`, and a `workflow_keyword_request` attachment on a
//! prompt that names ultracode. Each render reads only the lines added
//! since the last one; the position and the state live in a cache file.

use crate::git::fnv;
use memchr::memmem::Finder;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mark {
    /// Ultracode is on for the session.
    Session,
    /// Only the current prompt asked for it, with the `ultracode` keyword.
    Turn,
}

/// Cache files untouched this long are removed when a new one is made.
const MAX_AGE: Duration = Duration::from_secs(7 * 86_400);
/// A cache file in use is rewritten once it is this old, so that the
/// sweep never removes it.
const REFRESH: Duration = Duration::from_secs(86_400);
/// A long scan saves its progress this often, so that a render that
/// Claude Code stops early does not start the next one from the top.
const CHECKPOINT: u64 = 64 << 20;
/// The scan resumes only where the bytes just before it are unchanged.
const PRINT: u64 = 64;
/// Lines holding none of these are skipped without parsing.
const NEEDLES: [&str; 4] = [
    "ultra_effort_",
    "workflow_keyword_request",
    "\"promptSource\"",
    "<local-command-stdout>",
];
const STDOUT_TAG: &str = "<local-command-stdout>";
/// Models without xhigh effort, on which Claude Code does not run
/// ultracode (the list it checks for xhigh support).
const NO_ULTRACODE: [&str; 9] = [
    "claude-3",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-opus-4-5",
    "claude-opus-4-6",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
];

#[derive(Debug, Default, Clone, PartialEq)]
struct State {
    /// End of the last complete line read.
    offset: u64,
    /// Transcript length at the last scan.
    len: u64,
    /// Hash of the `PRINT` bytes before `offset`.
    print: u64,
    /// The latest record says ultracode is on.
    on: bool,
    /// promptId of the latest prompt.
    prompt: Option<String>,
    /// promptId of the latest prompt when it carried the keyword.
    kw: Option<String>,
}

fn opt(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("-")
}

fn encode(s: &State) -> String {
    format!(
        "{}\n{}\n{:016x}\n{}\n{}\n{}\n",
        s.offset,
        s.len,
        s.print,
        u8::from(s.on),
        opt(&s.prompt),
        opt(&s.kw)
    )
}

fn decode(text: &str) -> Option<State> {
    let mut l = text.lines();
    let offset = l.next()?.parse().ok()?;
    let len = l.next()?.parse().ok()?;
    let print = u64::from_str_radix(l.next()?, 16).ok()?;
    let on = match l.next()? {
        "1" => true,
        "0" => false,
        _ => return None,
    };
    let mut field = || l.next().map(|v| (v != "-").then(|| v.to_string()));
    Some(State {
        offset,
        len,
        print,
        on,
        prompt: field()?,
        kw: field()?,
    })
}

/// Hash of the bytes before `end`; leaves the position at `end`.
fn print<F: Read + Seek>(f: &mut F, end: u64) -> Option<u64> {
    let start = end.saturating_sub(PRINT);
    let mut buf = [0; PRINT as usize];
    let buf = &mut buf[..(end - start) as usize];
    f.seek(SeekFrom::Start(start)).ok()?;
    f.read_exact(buf).ok()?;
    Some(fnv(buf))
}

fn id(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| !s.is_empty() && !s.contains(char::is_control))
        .map(str::to_string)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
        } else if it.next() == Some('[') {
            for d in it.by_ref() {
                if ('@'..='~').contains(&d) {
                    break;
                }
            }
        }
    }
    out
}

/// Whether Claude Code runs ultracode on this model.
fn runs_ultracode(model: &str) -> bool {
    let id = model.to_ascii_lowercase().replace(['.', '_'], "-");
    !NO_ULTRACODE.iter().any(|n| {
        id.match_indices(n)
            .any(|(i, _)| !id[i + n.len()..].starts_with(|c: char| c.is_ascii_digit()))
    })
}

impl State {
    /// Read the complete lines after `offset`, streaming, and hand the
    /// state to `checkpoint` every `step` bytes.
    fn scan(&mut self, f: &mut File, step: u64, mut checkpoint: impl FnMut(&State)) {
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let finders: Vec<Finder> = NEEDLES.iter().map(Finder::new).collect();
        let mut r = BufReader::with_capacity(1 << 18, f);
        let mut line = Vec::new();
        let mut next = self.offset.saturating_add(step);
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(n) if line.last() == Some(&b'\n') => {
                    self.offset += n as u64;
                    if finders.iter().any(|x| x.find(&line).is_some()) {
                        self.record(&line);
                    }
                    if self.offset >= next {
                        next = self.offset.saturating_add(step);
                        let Some(p) = print(&mut r, self.offset) else {
                            break;
                        };
                        // A length below the file's makes the next render
                        // read on from here.
                        checkpoint(&State {
                            len: self.offset,
                            print: p,
                            ..self.clone()
                        });
                    }
                }
                _ => break,
            }
        }
    }

    /// Only the record type and the exact field decide, because tool
    /// results, hook output, API request copies and summaries quote the
    /// same strings.
    fn record(&mut self, line: &[u8]) {
        let Ok(r) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if r.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let s = |p: &str| r.pointer(p).and_then(Value::as_str);
        let text = match s("/type") {
            Some("attachment") => {
                match s("/attachment/type") {
                    Some("ultra_effort_enter") => self.on = true,
                    Some("ultra_effort_exit") => self.on = false,
                    Some("workflow_keyword_request") => self.kw = self.prompt.clone(),
                    _ => {}
                }
                return;
            }
            Some("user") if r.get("promptSource").is_some() => {
                self.prompt = id(r.get("promptId"));
                self.kw = None;
                return;
            }
            Some("user") => s("/message/content"),
            Some("system") if s("/subtype") == Some("local_command") => s("/content"),
            _ => None,
        };
        if let Some(out) = text.and_then(|t| t.strip_prefix(STDOUT_TAG)) {
            self.command(strip_ansi(out).trim_start());
        }
    }

    /// Output of `/effort`. Setting a plain level keeps ultracode as it
    /// is; the effort picker appends ` \u{b7} Ultracode on` or `off` when it
    /// changes ultracode as well.
    fn command(&mut self, t: &str) {
        self.on = if t.starts_with("Ultracode on (this session only)") || t.contains(" \u{b7} Ultracode on") {
            true
        } else if t.starts_with("Ultracode off.")
            || t.contains(" \u{b7} Ultracode off")
            || t.starts_with("Current effort level:")
            || t.starts_with("Effort level: auto")
        {
            false
        } else {
            return;
        };
    }
}

/// The marker for the transcript at `transcript`, given the model id and
/// the prompt id of this render.
pub fn mark(transcript: &Path, cache_dir: &Path, model: &str, prompt: Option<&str>) -> Option<Mark> {
    mark_with(
        transcript,
        cache_dir,
        model,
        prompt,
        SystemTime::now(),
        CHECKPOINT,
    )
}

fn mark_with(
    transcript: &Path,
    cache_dir: &Path,
    model: &str,
    prompt: Option<&str>,
    now: SystemTime,
    step: u64,
) -> Option<Mark> {
    let mut f = File::open(transcript).ok()?;
    let len = f.metadata().ok()?.len();
    let cache = cache_file(transcript, cache_dir);
    let tmp = cache.with_extension(format!("{}.tmp", std::process::id()));
    let read = read_cache(&cache, now);
    let old = read.as_ref().and_then(|(text, _)| decode(text));
    let kept = old
        .clone()
        .filter(|s| s.offset <= len && print(&mut f, s.offset) == Some(s.print));
    let fresh = kept.is_none();
    // Without a cache every render would read the whole transcript.
    if fresh && !writable(cache_dir, &tmp) {
        return None;
    }
    let mut st = kept.unwrap_or_default();
    if fresh || st.len != len {
        st.scan(&mut f, step, |s| save(&tmp, &cache, s));
        st.len = len;
        st.print = print(&mut f, st.offset).unwrap_or(0);
    }
    let stale = read.as_ref().is_some_and(|(_, stale)| *stale);
    if old.as_ref() != Some(&st) || stale {
        save(&tmp, &cache, &st);
    }
    if read.is_none() {
        sweep(cache_dir, now);
    }
    if st.on && runs_ultracode(model) {
        return Some(Mark::Session);
    }
    st.kw
        .as_deref()
        .is_some_and(|k| prompt == Some(k))
        .then_some(Mark::Turn)
}

fn cache_file(transcript: &Path, cache_dir: &Path) -> PathBuf {
    cache_dir.join(format!(
        "ultra-{:016x}",
        fnv(transcript.to_string_lossy().as_bytes())
    ))
}

/// The cache text, and whether it is due for a rewrite.
fn read_cache(path: &Path, now: SystemTime) -> Option<(String, bool)> {
    let mut f = File::open(path).ok()?;
    let stale = f
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .is_some_and(|age| age > REFRESH);
    let mut text = String::new();
    f.read_to_string(&mut text).ok()?;
    Some((text, stale))
}

fn writable(dir: &Path, tmp: &Path) -> bool {
    let ok = fs::create_dir_all(dir).is_ok() && File::create(tmp).is_ok();
    let _ = fs::remove_file(tmp);
    ok
}

fn save(tmp: &Path, file: &Path, st: &State) {
    if fs::write(tmp, encode(st)).is_err() || fs::rename(tmp, file).is_err() {
        let _ = fs::remove_file(tmp);
    }
}

fn sweep(dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let stale = e.file_name().to_string_lossy().starts_with("ultra-")
            && e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > MAX_AGE);
        if stale {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    const ON: &str = "Ultracode on (this session only): plans big tasks as workflows. Effort stays xhigh.";
    const OFF: &str = "Ultracode off. Effort stays xhigh.";
    const MODEL: &str = "claude-opus-5-5";

    struct Fix {
        dir: PathBuf,
        file: PathBuf,
    }

    impl Fix {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("csl-ultra-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let file = dir.join("t.jsonl");
            fs::write(&file, "").unwrap();
            Fix { dir, file }
        }
        fn raw(&self, s: &str) {
            let mut f = fs::OpenOptions::new().append(true).open(&self.file).unwrap();
            f.write_all(s.as_bytes()).unwrap();
        }
        fn add(&self, recs: &[Value]) {
            for r in recs {
                self.raw(&format!("{r}\n"));
            }
        }
        fn mark(&self, prompt: Option<&str>) -> Option<Mark> {
            mark(&self.file, &self.cache(), MODEL, prompt)
        }
        fn mark_on(&self, model: &str) -> Option<Mark> {
            mark(&self.file, &self.cache(), model, None)
        }
        fn mark_at(&self, now: SystemTime) -> Option<Mark> {
            mark_with(&self.file, &self.cache(), MODEL, None, now, CHECKPOINT)
        }
        fn cache(&self) -> PathBuf {
            self.dir.join("cache")
        }
        fn cache_file(&self) -> PathBuf {
            cache_file(&self.file, &self.cache())
        }
        fn state(&self) -> State {
            decode(&fs::read_to_string(self.cache_file()).unwrap()).unwrap()
        }
        fn len(&self) -> u64 {
            fs::metadata(&self.file).unwrap().len()
        }
        /// Rewrite the first enter record in place, keeping the length:
        /// only a scan from the top would notice.
        fn hide_enter(&self) {
            let body = fs::read_to_string(&self.file).unwrap();
            fs::write(
                &self.file,
                body.replacen("ultra_effort_enter", "ultra_effort_xxxxx", 1),
            )
            .unwrap();
        }
    }

    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn prompt_from(id: &str, text: &str, source: &str) -> Value {
        json!({"type": "user", "promptId": id, "promptSource": source, "isSidechain": false,
               "uuid": format!("u-{id}"), "message": {"role": "user", "content": text}})
    }

    fn prompt(id: &str, text: &str) -> Value {
        prompt_from(id, text, "typed")
    }

    fn attach(kind: &str) -> Value {
        json!({"type": "attachment", "isSidechain": false, "uuid": "a",
               "attachment": {"type": kind, "reminderType": "full"}})
    }

    fn stdout(text: &str) -> Value {
        json!({"type": "user", "isSidechain": false, "promptId": "cmd",
               "message": {"role": "user", "content": format!("{STDOUT_TAG}{text}</local-command-stdout>")}})
    }

    fn system_stdout(text: &str) -> Value {
        json!({"type": "system", "subtype": "local_command", "isSidechain": false, "level": "info",
               "content": format!("{STDOUT_TAG}{text}</local-command-stdout>")})
    }

    fn reply() -> Value {
        json!({"type": "assistant", "isSidechain": false, "uuid": "r",
               "message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]}})
    }

    /// Records that quote ultracode strings without being the real thing.
    fn traps(text: &str, kind: &str) -> Vec<Value> {
        let quoted = format!(
            "{STDOUT_TAG}{text}</local-command-stdout> {{\"type\":\"attachment\",\"attachment\":{{\"type\":\"{kind}\"}}}}"
        );
        vec![
            json!({"type": "api-request-blob", "body": quoted}),
            json!({"type": "api-request", "messages": [{"role": "user", "content": quoted}]}),
            json!({"type": "user", "isSidechain": false, "promptId": "p1",
                   "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": quoted}]},
                   "toolUseResult": {"stdout": quoted}}),
            json!({"type": "attachment", "isSidechain": false,
                   "attachment": {"type": "hook_success", "hookName": "PostToolUse", "stdout": quoted, "content": kind}}),
            json!({"type": "user", "isSidechain": false, "isCompactSummary": true,
                   "message": {"role": "user", "content": format!("This session is being continued from a previous conversation. {quoted}")}}),
            json!({"type": "attachment", "isSidechain": true, "attachment": {"type": kind}}),
            json!({"type": "user", "isSidechain": true, "promptId": "sub",
                   "message": {"role": "user", "content": format!("{STDOUT_TAG}{text}</local-command-stdout>")}}),
            json!({"type": "system", "subtype": "local_command", "isSidechain": true,
                   "content": format!("{STDOUT_TAG}{text}</local-command-stdout>")}),
        ]
    }

    #[test]
    fn enter_then_exit() {
        let t = Fix::new("enter");
        t.add(&[prompt("p1", "hi")]);
        assert_eq!(t.mark(Some("p1")), None);
        t.add(&[attach("ultra_effort_enter")]);
        assert_eq!(t.mark(Some("p1")), Some(Mark::Session));
        // Enter is not repeated at every prompt.
        t.add(&[prompt("p2", "more"), attach("output_style")]);
        assert_eq!(t.mark(Some("p2")), Some(Mark::Session));
        t.add(&[prompt("p3", "next"), attach("ultra_effort_exit")]);
        assert_eq!(t.mark(Some("p3")), None);
    }

    #[test]
    fn effort_command_toggles() {
        let t = Fix::new("toggle");
        t.add(&[stdout(ON)]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        t.add(&[system_stdout(&format!("\x1b[2m{OFF}\x1b[22m"))]);
        assert_eq!(t.mark(None), None);
        t.add(&[system_stdout(&format!("\x1b[2m{ON}\x1b[22m"))]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        // Refusals leave the state alone.
        t.add(&[
            stdout("Ultracode isn't available on claude-opus-4-6. Valid options are: low, medium, high"),
            stdout("Ultracode needs dynamic workflows enabled (see /config). Valid options are: low"),
        ]);
        assert_eq!(t.mark(None), Some(Mark::Session));
    }

    #[test]
    fn plain_level_keeps_ultracode() {
        let t = Fix::new("plain");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        for msg in [
            "Set effort level to high (saved as your default for new sessions): \
             Comprehensive implementation with extensive testing",
            "Set effort level to max (this session only): Maximum capability with deepest reasoning",
            "Effort level set to auto (this session only)",
            "Effort 'max' exceeds the cap for claude-opus-5-5 set by your settings or organization; \
             set to 'xhigh' instead: Deepest",
            "Kept effort level as high",
        ] {
            t.add(&[stdout(msg)]);
            assert_eq!(t.mark(None), Some(Mark::Session), "{msg}");
        }
        // Prompts after it write nothing new while ultracode stays on.
        t.add(&[prompt("p2", "y"), prompt("p3", "z")]);
        assert_eq!(t.mark(Some("p3")), Some(Mark::Session));
    }

    #[test]
    fn off_suffix_turns_it_off() {
        let t = Fix::new("offsuffix");
        for msg in [
            "Set effort level to high (this session only): Balanced \u{b7} Ultracode off",
            "Effort level set to auto (this session only) \u{b7} Ultracode off",
            "Effort 'max' exceeds the cap for claude-opus-5-5 set by your settings or organization; \
             set to 'xhigh' instead: Deepest \u{b7} Ultracode off",
            "\x1b[2mSet effort level to low: Fast\x1b[22m\x1b[2m \u{b7} Ultracode off\x1b[22m",
        ] {
            t.add(&[stdout(ON)]);
            assert_eq!(t.mark(None), Some(Mark::Session));
            t.add(&[stdout(msg)]);
            assert_eq!(t.mark(None), None, "{msg}");
        }
    }

    #[test]
    fn effort_status_lines() {
        let t = Fix::new("status");
        for (msg, want) in [
            (
                "Current effort level: xhigh (Deepest) \u{b7} Ultracode on",
                Some(Mark::Session),
            ),
            ("Current effort level: xhigh (Deepest)", None),
            (
                "Effort level: auto (currently xhigh) \u{b7} Ultracode on",
                Some(Mark::Session),
            ),
            ("Effort level: auto", None),
            ("Effort level: auto \u{b7} Ultracode on", Some(Mark::Session)),
            (
                "Effort level: auto (currently high, set by your organization)",
                None,
            ),
            // The effort picker, level and ultracode changed together.
            (
                "Set effort level to high (this session only): \
                 Comprehensive implementation with extensive testing \u{b7} Ultracode on",
                Some(Mark::Session),
            ),
            (
                "Effort level set to auto (this session only) \u{b7} Ultracode on",
                Some(Mark::Session),
            ),
            (
                "\x1b[2mSet effort level to max (saved as your default for new sessions): Maximum\x1b[22m \
                 \u{b7} Ultracode on",
                Some(Mark::Session),
            ),
            (
                "Set effort level to low (this session only): Fast \u{b7} Ultracode off",
                None,
            ),
        ] {
            // Start from the other state, so that each line has to move it.
            let (start, before) = match want {
                Some(_) => (OFF, None),
                None => (ON, Some(Mark::Session)),
            };
            t.add(&[stdout(start)]);
            assert_eq!(t.mark(None), before, "{msg}");
            t.add(&[stdout(msg)]);
            assert_eq!(t.mark(None), want, "{msg}");
        }
    }

    #[test]
    fn keyword_turn_lasts_one_prompt() {
        let t = Fix::new("keyword");
        t.add(&[
            prompt("p1", "can ultracode em nhe"),
            attach("workflow_keyword_request"),
            attach("output_style"),
        ]);
        assert_eq!(t.mark(Some("p1")), Some(Mark::Turn));
        assert_eq!(t.mark(Some("p0")), None);
        // A subagent's records do not end the turn.
        t.add(&[
            json!({"type": "user", "isSidechain": true, "promptId": "sub", "promptSource": "typed",
                       "message": {"role": "user", "content": "task"}}),
        ]);
        assert_eq!(t.mark(Some("p1")), Some(Mark::Turn));
        t.add(&[prompt("p2", "thanks")]);
        assert_eq!(t.mark(Some("p2")), None);
        assert_eq!(t.mark(Some("p1")), None);
        // A task notification starts a new prompt too.
        t.add(&[prompt("p3", "ultracode this"), attach("workflow_keyword_request")]);
        assert_eq!(t.mark(Some("p3")), Some(Mark::Turn));
        t.add(&[prompt_from(
            "p4",
            "<task-notification>done</task-notification>",
            "system",
        )]);
        assert_eq!(t.mark(Some("p4")), None);
        // The session marker wins over the keyword.
        t.add(&[
            prompt("p5", "ultracode"),
            attach("workflow_keyword_request"),
            stdout(ON),
        ]);
        assert_eq!(t.mark(Some("p5")), Some(Mark::Session));
    }

    #[test]
    fn keyword_needs_the_prompt_id() {
        let t = Fix::new("kwprompt");
        // Claude Code sends no prompt_id before the first prompt of the
        // process, as after a resume: no turn of this process is running.
        t.add(&[
            prompt("p1", "ultracode this"),
            attach("workflow_keyword_request"),
            reply(),
        ]);
        assert_eq!(t.mark(None), None);
        assert_eq!(t.mark(Some("p1")), Some(Mark::Turn));
        // A keyword whose prompt is not known matches no prompt id.
        let u = Fix::new("kwunknown");
        u.add(&[attach("workflow_keyword_request")]);
        assert_eq!(u.mark(None), None);
        assert_eq!(u.mark(Some("p1")), None);
    }

    #[test]
    fn model_without_ultracode_hides_it() {
        let t = Fix::new("model");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        assert_eq!(t.mark_on("claude-opus-5-5"), Some(Mark::Session));
        // /model writes nothing about ultracode; the next prompt writes exit.
        t.add(&[stdout("Set model to Sonnet 4.6 (claude-sonnet-4-6)")]);
        assert_eq!(t.mark_on("claude-sonnet-4-6"), None);
        assert_eq!(t.mark_on("claude-opus-5-5[1m]"), Some(Mark::Session));
        // The keyword turn does not depend on the model.
        t.add(&[prompt("p2", "ultracode"), attach("workflow_keyword_request")]);
        assert_eq!(
            mark(&t.file, &t.cache(), "claude-sonnet-4-6", Some("p2")),
            Some(Mark::Turn)
        );
    }

    #[test]
    fn models_that_run_ultracode() {
        for m in [
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-fable-5",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "us.anthropic.claude-opus-5-5-v1:0",
            "",
        ] {
            assert!(runs_ultracode(m), "{m}");
        }
        for m in [
            "claude-opus-4-6",
            "claude-opus-4-6[1m]",
            "claude-opus-4-5-20251101",
            "claude-sonnet-4-6",
            "Claude-Sonnet-4-6",
            "claude-sonnet-4-5@20250929",
            "us.anthropic.claude-opus-4-6-v1:0",
            "claude-3-7-sonnet-20250219",
            "claude-3-5-haiku-latest",
            "claude-haiku-4-5",
            "claude-opus-4-1",
        ] {
            assert!(!runs_ultracode(m), "{m}");
        }
    }

    #[test]
    fn traps_do_not_count() {
        let t = Fix::new("traps");
        t.add(&[prompt("p1", "look")]);
        t.add(&traps(ON, "ultra_effort_enter"));
        t.add(&traps("x", "workflow_keyword_request"));
        assert_eq!(t.mark(Some("p1")), None);
        assert_eq!(t.mark(None), None);
        t.add(&[attach("ultra_effort_enter")]);
        t.add(&traps(OFF, "ultra_effort_exit"));
        t.add(&traps(
            "Current effort level: xhigh (Deepest)",
            "ultra_effort_exit",
        ));
        assert_eq!(t.mark(Some("p1")), Some(Mark::Session));
    }

    #[test]
    fn reads_only_new_lines() {
        let t = Fix::new("incr");
        t.add(&[
            prompt("p1", "x"),
            attach("ultra_effort_enter"),
            prompt("p2", &"y".repeat(200)),
        ]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        assert_eq!((t.state().offset, t.state().len), (t.len(), t.len()));
        t.hide_enter();
        t.add(&[prompt("p3", "z")]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        assert_eq!(t.state().offset, t.len());
        // Nothing new: the cache is left as it is.
        let before = t.state();
        assert_eq!(t.mark(None), Some(Mark::Session));
        assert_eq!(t.state(), before);
    }

    #[test]
    fn partial_last_line_waits() {
        let t = Fix::new("partial");
        t.add(&[prompt("p1", "x")]);
        let done = t.len();
        let rec = attach("ultra_effort_enter").to_string();
        let (a, b) = rec.split_at(rec.len() / 2);
        t.raw(a);
        assert_eq!(t.mark(None), None);
        assert_eq!(t.state().offset, done);
        t.raw(b);
        assert_eq!(t.mark(None), None, "still no newline");
        t.raw("\n");
        assert_eq!(t.mark(None), Some(Mark::Session));
        assert_eq!(t.state().offset, t.len());
    }

    #[test]
    fn truncation_and_rewrite_rescan() {
        let t = Fix::new("rewrite");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        // Truncated below the resume point.
        fs::write(&t.file, format!("{}\n", prompt("p1", "x"))).unwrap();
        assert_eq!(t.mark(None), None);
        t.add(&[attach("ultra_effort_enter")]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        // Replaced by a longer file with other bytes before the resume point.
        let at = t.state().offset as usize;
        fs::write(&t.file, format!("{}\n", prompt("q1", &"w".repeat(at + 100)))).unwrap();
        assert_eq!(t.mark(None), None);
        assert_eq!(t.state().prompt.as_deref(), Some("q1"));
    }

    #[test]
    fn long_scan_saves_progress() {
        let t = Fix::new("checkpoint");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        for i in 0..40 {
            t.add(&[reply(), prompt(&format!("f{i}"), &"y".repeat(200))]);
        }
        let mut f = File::open(&t.file).unwrap();
        let mut saved = Vec::new();
        let mut st = State::default();
        st.scan(&mut f, 1000, |s| saved.push(s.clone()));
        assert!(saved.len() >= 8, "{}", saved.len());
        for s in &saved {
            assert!(s.on);
            assert_eq!(s.len, s.offset, "the next render reads on");
            assert_eq!(print(&mut f, s.offset), Some(s.print));
        }
        // The seeks for the checkpoints do not disturb the scan.
        let mut whole = State::default();
        whole.scan(&mut File::open(&t.file).unwrap(), u64::MAX, |_| panic!());
        assert_eq!((st.offset, &st.prompt), (t.len(), &whole.prompt));
        assert_eq!(whole.prompt.as_deref(), Some("f39"));

        // A render stopped after the first checkpoint left it in the cache.
        // The next one reads on from there and does not start from the top,
        // which would miss the hidden enter record.
        fs::create_dir_all(t.cache()).unwrap();
        fs::write(t.cache_file(), encode(&saved[0])).unwrap();
        t.hide_enter();
        assert_eq!(t.mark(None), Some(Mark::Session));
        assert_eq!(t.state().offset, t.len());
        assert_eq!(t.state().prompt.as_deref(), Some("f39"));
    }

    #[test]
    fn unwritable_cache_reads_nothing() {
        let t = Fix::new("unwritable");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        // A regular file where the cache folder should be.
        let blocked = t.dir.join("blocked");
        fs::write(&blocked, "x").unwrap();
        let cache = blocked.join("claude-statusline");
        for _ in 0..2 {
            assert_eq!(mark(&t.file, &cache, MODEL, Some("p1")), None);
        }
        assert_eq!(fs::read_to_string(&blocked).unwrap(), "x");
    }

    #[test]
    fn cache_in_use_is_refreshed() {
        let t = Fix::new("refresh");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        // A trailing line the decoder ignores shows whether the file was
        // written again.
        let tagged = format!("{}junk\n", encode(&t.state()));
        fs::write(t.cache_file(), &tagged).unwrap();
        let now = SystemTime::now();
        assert_eq!(t.mark_at(now), Some(Mark::Session));
        assert_eq!(fs::read_to_string(t.cache_file()).unwrap(), tagged, "nothing new");
        assert_eq!(t.mark_at(now + REFRESH * 2), Some(Mark::Session));
        assert_eq!(fs::read_to_string(t.cache_file()).unwrap(), encode(&t.state()));
    }

    #[test]
    fn garbled_cache_is_rebuilt() {
        let t = Fix::new("garbled");
        t.add(&[prompt("p1", "x"), attach("ultra_effort_enter")]);
        assert_eq!(t.mark(None), Some(Mark::Session));
        for junk in ["junk\n", "1\n2\n3\nXHIGH\n-\n-\n"] {
            fs::write(t.cache_file(), junk).unwrap();
            assert_eq!(t.mark(None), Some(Mark::Session));
            assert_eq!(t.state().offset, t.len());
        }
    }

    #[test]
    fn cache_round_trip() {
        let s = State {
            offset: 120,
            len: 130,
            print: 0xdead_beef,
            on: true,
            prompt: None,
            kw: Some("p1".into()),
        };
        assert_eq!(decode(&encode(&s)), Some(s));
        assert_eq!(decode(&encode(&State::default())), Some(State::default()));
        assert_eq!(decode("1\n2\nzz\n0\n-\n-\n"), None);
        assert_eq!(decode("1\n2\n3\n0\n-\n"), None);
        assert_eq!(decode("1\n2\n3\nXHIGH\n-\n-\n"), None, "earlier format");
    }

    #[test]
    fn missing_transcript_shows_nothing() {
        let t = Fix::new("missing");
        let gone = t.dir.join("gone.jsonl");
        assert_eq!(mark(&gone, &t.cache(), MODEL, None), None);
        assert!(!t.cache().exists());
    }

    #[test]
    fn old_cache_files_are_swept() {
        let t = Fix::new("sweep");
        let dir = t.cache();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ultra-0000000000000001"), "x").unwrap();
        fs::write(dir.join("git-0000000000000001"), "x").unwrap();
        sweep(&dir, SystemTime::now());
        assert!(dir.join("ultra-0000000000000001").exists(), "fresh file kept");
        sweep(&dir, SystemTime::now() + MAX_AGE + Duration::from_secs(60));
        assert!(!dir.join("ultra-0000000000000001").exists());
        assert!(dir.join("git-0000000000000001").exists(), "other caches kept");
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(
            strip_ansi("\x1b[2mUltracode\x1b[22m off\x1b[0m."),
            "Ultracode off."
        );
        assert_eq!(strip_ansi("a\x1b[38;5;139mb"), "ab");
    }
}
