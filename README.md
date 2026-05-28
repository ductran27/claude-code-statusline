# claude-code-statusline

Personal Claude Code statusline — single-row, tight separators, muted 256-color palette.

## Format

```
[plan-icon ⚡] [project]│[git branch + dirty/ahead/behind]│tokens/ctx│5h pct% (reset)/7d pct% (reset)│Model (ctx|EFFORT[|FAST])│[output-style]
```

Example:

```
ductran│280k/1M│5h 10% (2h36m)/7d 2% (Tue 12PM)│Opus 4.8 (1M|MAX)
```

With fast mode on: `Opus 4.8 (1M|MAX|FAST)` — the `FAST` marker is appended only when active.

## Separators

- `│` between segments — no surrounding spaces
- `/` between 5h and 7d — no surrounding spaces
- `|` inside `(ctx|EFFORT)` — no surrounding spaces
- Inner spaces kept for readability: `5h 10%`, `branch ✓`, `(1M|MAX)`

## Color thresholds

- Tokens: green ≤30 / yellow ≤60 / orange ≤80 / red >80 (applied to `tokens/ctx`)
- Rate limits: green ≤50 / yellow ≤75 / orange ≤90 / red >90

## Palette

Muted 256-color: sage 108, gold 179, terra cotta 173, brick 167, slate 103, mauve 139, rose 181, warm gray 245, white-dim 250.

Bold reserved for the effort tier label (MAX/XHIGH/HIGH/etc.).

## Effort tier & fast mode

The active effort tier is read straight from the statusLine stdin JSON `.effort.level`
(Claude Code ≥ ~2.1.154). It is **live** — reflects mid-session `/effort` and `/model`
toggles in real time — and covers Opus 4.8's tiers `low | medium | high | xhigh | max`
(`ultra` = ultracode). The raw value is uppercased and rendered bold
(`xhigh` → **XHIGH**, `max` → **MAX**).

Fallback (only when `.effort.level` is absent — older builds, or models without an
effort parameter): `~/.claude/settings.json` → `.effortLevel`. Note that `settings.json`
cannot store `max` (session-only), so reading `.effort.level` is what makes a live **MAX**
show up at all.

**Fast mode** (`/fast`) is a separate speed axis, read live from the top-level
`.fast_mode` boolean. When active, a non-bold terra-cotta `FAST` marker is appended:
`(1M|MAX|FAST)`. Nothing is shown when fast mode is off.

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
