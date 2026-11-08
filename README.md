# claude-code-statusline

A status line for Claude Code, written in Rust. It is a single small binary
for macOS, Linux and Windows, renders in about 3 ms, and shows only what
Claude Code itself reports.

```
proj│main ✓ ↑1│92k/1M│5h 28% (2h36m)/7d 71% (Sun 2AM)/F 12%│Opus 5.5 (1M|MAX|FAST)│Concise
```

| Segment | Source |
|---|---|
| `proj` | folder name of `workspace.project_dir` |
| `main ✓ ↑1 ↓2` | one `git status --porcelain=v2 --branch`; `✗` when the tree has changes |
| `92k/1M` | context in use and window size, counted as Claude Code counts it |
| `5h 28% (2h36m)` | `rate_limits.five_hour`, with the time left until reset |
| `7d 71% (Sun 2AM)` | `rate_limits.seven_day`; past a day the reset shows as local day and hour |
| `F 12%` | per-model weekly bucket (the Fable row of `/usage`), from claude-swap |
| `Opus 5.5` | `model.display_name`, or a label parsed from `model.id` |
| `(1M\|MAX\|FAST)` | context window, effort tier, fast mode |
| `Concise` | output style, hidden when it is the default |

Segments without data are left out. Before the first reply of a session
there are no rate limits yet, and models without an effort setting show no
tier.

## Effort tier

The tier is `effort.level` from the JSON that Claude Code sends to the
status line. Claude Code fills it with the effort it will send on the next
request: one of `low`, `medium`, `high`, `xhigh` or `max`, already lowered
to what the model supports (for example `max` becomes `high` on a model
without max effort). It follows `/effort`, `/model` and `--effort` at once.

Two things the earlier shell version did wrong are gone:

- When `effort` was missing it fell back to `effortLevel` in settings.json
  and showed, say, `XHIGH` for a model that takes no effort at all. Now no
  tier is shown, because none is sent.
- It had an `ULTRA` tier. Claude Code never reports one: ultracode is a
  workflow mode that runs on top of an ordinary effort level
  (`/effort ultracode` sets `xhigh`), so the tier shown is that level.

Per turn effort that a keyword adds to a single prompt is not part of the
status line JSON and is not shown.

Tier colors, all bold: LOW gray, MEDIUM light gray, HIGH sage, XHIGH gold,
MAX brick. FAST is terra cotta and not bold.

## Per-model weekly bucket

The status line JSON carries only the account wide 5h and 7d windows. The
per-model weekly buckets come from
[claude-swap](https://github.com/realiti4/claude-swap), which keeps the last
reading of every managed account in `cache/usage.json`
(`~/.claude-swap-backup` on macOS and Windows, `~/.local/share/claude-swap`
on Linux). The binary reads that file directly, so a render never starts
Python or touches the network.

The row shown is the one whose 7d reset matches the 7d reset in the status
line JSON, which is this session's own account even when claude-swap has
switched the active account since the session started. Without rate limits
in the JSON, the account logged in to Claude Code is used. A bucket whose
reset time has passed is hidden rather than shown with an old number. When
the matching reading is more than 10 minutes old, `cswap status --json` is
started in the background, at most once every 5 minutes, so the next render
has fresh data. Without claude-swap the segment does not appear.

## Speed

Measured on an M-series Mac, from spawn to exit:

| | per render |
|---|---|
| earlier shell version | 127 ms |
| this binary | 3 to 5 ms |
| process spawn alone | 1.7 ms |

The git result is cached per repository for 5 seconds and dropped as soon
as git rewrites the index, HEAD or refs, so commits, staging and branch
switches show at once and edits show within 5 seconds. Git runs with
`--no-optional-locks`, so it never competes with your own git commands for
the index lock. A repository where git hangs (network drive, huge untracked
tree) is given 1.5 s, then the branch is read from HEAD and git is not tried
again for a minute.

## Install

With Rust installed (https://rustup.rs), from a clone:

```sh
./install.sh          # macOS, Linux, Git Bash on Windows
```

```powershell
.\install.ps1         # Windows PowerShell
```

Without Rust, the same scripts download the latest release binary. Both put
the binary in `~/.claude/bin` and print the settings block to add to
`~/.claude/settings.json`:

```json
"statusLine": {
  "type": "command",
  "command": "~/.claude/bin/claude-statusline",
  "refreshInterval": 60
}
```

On Windows use the full path the installer prints, for example
`C:/Users/you/.claude/bin/claude-statusline.exe`. `refreshInterval` keeps
the reset countdowns current while the session is idle; it costs one 3 ms
run a minute.

## Options

Environment variables, set where Claude Code is started:

| Variable | Effect |
|---|---|
| `STATUSLINE_GIT=off` | no git segment |
| `STATUSLINE_SCOPED=off` | no per-model bucket |
| `SMARTBAR_CSWAP=/path/to/cswap` | cswap used for background refreshes |
| `STATUSLINE_CSWAP_USAGE=/path/usage.json` | claude-swap usage file to read |

Small cache files live in `~/.cache/claude-statusline` (`%LOCALAPPDATA%`
on Windows).

## Development

```sh
cargo test --release      # unit tests and end-to-end runs of the binary
cargo clippy --all-targets -- -D warnings
```

CI runs both on macOS, Linux and Windows. Pushing a `v*` tag builds release
binaries for macOS (arm64, x86_64), Linux (static musl, x86_64 and arm64)
and Windows (x86_64).
