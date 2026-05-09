# claude-code-statusline

Personal Claude Code statusline — single-row, tight separators, muted 256-color palette.

## Format

```
[plan-icon ⚡] [project]│[git branch + dirty/ahead/behind]│tokens/ctx│5h pct% (reset)/7d pct% (reset)│Model (ctx|EFFORT)│[output-style]
```

Example:

```
ductran│280k/1M│5h 10% (2h36m)/7d 2% (Tue 12PM)│Opus 4.7 (1M|MAX)
```

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

## Effort tier

Source priority for the active tier:

1. Latest `[1m<level>[22m effort` line in the current session transcript (live `/model` toggles).
2. `~/.claude/settings.json` → `.effortLevel` (persistent default).

Raw value is uppercased and rendered bold (`xhigh` → **XHIGH**, `max` → **MAX**).

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
