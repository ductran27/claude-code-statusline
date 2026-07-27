# claude-code-statusline

Personal Claude Code statusline — single-row, tight separators, muted 256-color palette.

## Format

```
[plan-icon ⚡] [project]│[git branch + dirty/ahead/behind]│tokens/ctx│5h pct% (reset)/7d pct% (reset)/F pct%│Model (ctx|EFFORT[|FAST])│[output-style]
```

Example:

```
ductran│170k/1M│5h 37% (1h25m)/7d 28% (Mon 1AM)/F 39%│Fable 5 (1M|MAX)
```

With fast mode on: `Fable 5 (1M|MAX|FAST)` — the `FAST` marker is appended only when active.

## Separators

- `│` between segments — no surrounding spaces
- `/` between the rate buckets (5h, 7d, per-model) — no surrounding spaces
- `|` inside `(ctx|EFFORT)` — no surrounding spaces
- Inner spaces kept for readability: `5h 10%`, `branch ✓`, `(1M|MAX)`

## Model names

`.model.display_name` from the statusLine stdin is preferred (`Fable 5`,
`Opus 4.8`, `Haiku 4.5`) — zero maintenance as new families ship; a leading
`Claude ` is stripped to stay tight. Builds without `display_name` fall back
to parsing `.model.id`: family from the slug (fable/opus/sonnet/haiku),
version from the first `X-Y` digit pair (`4-8` → 4.8, legacy
`claude-3-7-sonnet` → 3.7) or a lone trailing digit group
(`claude-fable-5` / `claude-opus-5` → 5). Settings-style ids carrying a
context suffix (`claude-fable-5[1m]`) have it stripped before parsing.

## Color thresholds

- Tokens: green ≤30 / yellow ≤60 / orange ≤80 / red >80 (applied to `tokens/ctx`)
- Rate limits: green ≤50 / yellow ≤75 / orange ≤90 / red >90

Rate percentages are **rounded**, not ceiled — the API emits float artifacts
(`28.000000000000004`) that ceil inflated to 29% while `/usage` said 28%.
The token count is the exact `context_window.current_usage` sum (input +
output + cache) when the build provides it, else derived from size × used%.

## Palette

Muted 256-color: sage 108, gold 179, terra cotta 173, brick 167, slate 103, mauve 139, rose 181, warm gray 245, white-dim 250.

Bold reserved for the effort tier label (MAX/ULTRA/XHIGH/etc.).

## Effort tier & fast mode

The active effort tier is read straight from the statusLine stdin JSON `.effort.level`
(Claude Code ≥ ~2.1.154). It is **live** — reflects mid-session `/effort` and `/model`
toggles in real time — and covers `low | medium | high | xhigh | max`, plus
**`ultra`** when ultracode is on. The raw value is uppercased and rendered bold,
in a per-tier color that heats up as the tier climbs:

| tier | color |
|---|---|
| LOW | warm gray 245 |
| MEDIUM | white-dim 250 |
| HIGH | sage 108 |
| XHIGH | gold 179 |
| MAX | brick 167 |
| ULTRA | mauve 139 |

Unknown future tiers render bold-uncolored. Terra cotta is reserved for the
`FAST` marker so the two axes never blur.

Fallback (only when `.effort.level` is absent — older builds, or models without an
effort parameter): `~/.claude/settings.json` → `.effortLevel`. Note that `settings.json`
cannot store `max` or `ultra` (session-only), so reading `.effort.level` is what makes
a live **MAX**/**ULTRA** show up at all.

**Fast mode** (`/fast`) is a separate speed axis, read live from the top-level
`.fast_mode` boolean. When active, a non-bold terra-cotta `FAST` marker is appended:
`(1M|MAX|FAST)`. Nothing is shown when fast mode is off.

## Per-model weekly % (via claude-swap)

The statusLine stdin only carries the account-wide 5h/7d numbers. The
per-model weekly buckets — the "Fable 5" row in `/usage` — come from
[claude-swap](https://github.com/realiti4/claude-swap)'s last-good local data
(`cswap list --json`), which [AI smartbar](https://github.com/ductran27/AI_smartbar)
keeps fresh wherever its tray is polling. Each bucket of the **active**
account renders as a single-letter label after 7d: `F 39%` (Fable → F,
colored by the rate thresholds).

- Renders never block: they read `~/.cache/claude-statusline/cswap.json`; a
  background refresh runs at most once per 90 s (`SCOPED_TTL`). Only the
  first-ever render (no cache yet) refreshes synchronously (~0.4 s).
- The bucket's reset time is shown only when it deviates >1 h from the 7d
  reset — they normally tick together, so repeating the timestamp is noise.
- No `cswap` on the machine → the segment simply never appears.
- `STATUSLINE_SCOPED=off` disables it; `SMARTBAR_CSWAP=/path/to/cswap`
  pins the binary (same variable AI smartbar honors).

The stdin 5h/7d are per-render live; the cswap snapshot refreshes on the
smartbar's cadence — the two can briefly disagree by a point or two.

## Install

1. Copy the script:

   ```sh
   cp statusline-cool.sh ~/.claude/statusline-cool.sh
   chmod +x ~/.claude/statusline-cool.sh
   ```

2. Wire it up in `~/.claude/settings.json`:

   ```json
   {
     "statusLine": {
       "type": "command",
       "command": "~/.claude/statusline-cool.sh"
     }
   }
   ```

3. Restart Claude Code.
