#!/bin/bash

# Statusline for Claude Code — clean, information-dense, dynamically colored
# Format: Icon | Project | Git | Tokens | Rate Limits | Model | Style
#
# Percentages are color-coded (green → yellow → orange → red) to carry the
# threshold signal. Example rate limits segment:
#   5h 10% (2h36m) / 7d 2% (Thu 5PM) / F 39%
# ("F 39%" is the per-model weekly bucket — e.g. Fable — sourced from cswap;
#  see the scoped-buckets section below.)

# ═══════════════════════════════════════════════════
# ✏️  CUSTOMIZE HERE
# ═══════════════════════════════════════════════════

# Plan icon — shown as the first element in the statusline.
# Change this to any emoji or short text you like.
PLAN_ICON="⚡"

# Color thresholds for token usage bar (percentage)
TOKEN_THRESH_GREEN=30    # green  ≤ this
TOKEN_THRESH_YELLOW=60   # yellow ≤ this
TOKEN_THRESH_ORANGE=80   # orange ≤ this, red above

# Color thresholds for rate limit bars (percentage)
RATE_THRESH_GREEN=50     # green  ≤ this
RATE_THRESH_YELLOW=75    # yellow ≤ this
RATE_THRESH_ORANGE=90    # orange ≤ this, red above

# Per-model weekly buckets (the "Fable" row in /usage) via claude-swap.
# Renders always read a cache; a background refresh runs at most once per
# SCOPED_TTL seconds. STATUSLINE_SCOPED=off disables the segment entirely;
# SMARTBAR_CSWAP points at a specific cswap binary (same var AI smartbar uses).
SCOPED_TTL=90

# ═══════════════════════════════════════════════════
# ANSI colors (256-color; works in most modern terminals)
# ═══════════════════════════════════════════════════
c_reset="\033[0m"
c_dim="\033[2m"
c_bold="\033[1m"
c_green="\033[38;5;108m"      # soft sage
c_yellow="\033[38;5;179m"     # muted gold
c_orange="\033[38;5;173m"     # soft terra cotta
c_red="\033[38;5;167m"        # dusty brick (gentler than 196)
c_cyan="\033[38;5;103m"       # slate blue
c_magenta="\033[38;5;139m"    # dusty mauve
c_pink="\033[38;5;181m"       # soft rose
c_gray="\033[38;5;245m"       # warm gray
c_white_dim="\033[38;5;250m"

sep="${c_dim}${c_gray}│${c_reset}"

# ═══════════════════════════════════════════════════
# Parse JSON input from Claude Code
# ═══════════════════════════════════════════════════
input=$(cat)

# used_percentage arrives as a float on some builds — floor it so the integer
# comparisons below never error out. Rate-limit percentages are rounded, not
# ceiled: the API emits float artifacts (28.000000000000004) that ceil would
# inflate to 29% while /usage shows 28%. Token counts prefer the exact
# current_usage sums over back-computing size × rounded-percentage.
IFS='|' read -r model_id context_size used_pct project_dir output_style \
  five_hour_pct five_hour_reset seven_day_pct seven_day_reset effort_level fast_mode \
  model_display ctx_tokens \
  <<< "$(echo "$input" | jq -r '[
    (.model.id // "unknown"),
    (.context_window.context_window_size // 0),
    ((.context_window.used_percentage // 0) | floor),
    (.workspace.project_dir // ""),
    (.output_style.name // "default"),
    (if .rate_limits.five_hour.used_percentage != null then (.rate_limits.five_hour.used_percentage | round) else "" end),
    (.rate_limits.five_hour.resets_at // ""),
    (if .rate_limits.seven_day.used_percentage != null then (.rate_limits.seven_day.used_percentage | round) else "" end),
    (.rate_limits.seven_day.resets_at // ""),
    (.effort.level // ""),
    (.fast_mode // false),
    (.model.display_name // ""),
    (if (.context_window.current_usage | type) == "object" then
       ((.context_window.current_usage.input_tokens // 0)
        + (.context_window.current_usage.output_tokens // 0)
        + (.context_window.current_usage.cache_creation_input_tokens // 0)
        + (.context_window.current_usage.cache_read_input_tokens // 0))
     else 0 end)
  ] | join("|")')"

model_id="${model_id:-unknown}"
context_size="${context_size:-0}"
used_pct="${used_pct:-0}"
output_style="${output_style:-default}"

# ═══════════════════════════════════════════════════
# Helpers
# ═══════════════════════════════════════════════════

format_tokens() {
  local n=${1:-0}
  if [ "$n" -ge 1000000 ] 2>/dev/null; then
    printf "%.1fm" "$(echo "$n / 1000000" | bc -l)"
  elif [ "$n" -ge 1000 ] 2>/dev/null; then
    echo "$((n / 1000))k"
  else
    echo "$n"
  fi
}

format_context() {
  local n=${1:-0}
  if [ "$n" -ge 1000000 ] 2>/dev/null; then
    printf "%.0fM" "$(echo "$n / 1000000" | bc -l)"
  elif [ "$n" -ge 1000 ] 2>/dev/null; then
    echo "$((n / 1000))K"
  else
    echo "$n"
  fi
}

format_reset() {
  local reset_epoch=${1:-0}
  [ -z "$reset_epoch" ] || [ "$reset_epoch" = "0" ] && return
  local now diff minutes hours rem_min
  now=$(date +%s)
  diff=$(( reset_epoch - now ))
  [ "$diff" -le 0 ] && { echo "now"; return; }
  minutes=$(( diff / 60 ))
  hours=$(( minutes / 60 ))
  rem_min=$(( minutes % 60 ))
  if [ "$hours" -eq 0 ]; then
    echo "${minutes}m"
  elif [ "$hours" -lt 24 ]; then
    echo "${hours}h${rem_min}m"
  else
    if [[ "$OSTYPE" == darwin* ]]; then
      date -r "$reset_epoch" "+%a %-I%p"
    else
      date -d "@$reset_epoch" "+%a %-I%p"
    fi
  fi
}

# Seconds since a file was modified; a huge number when it doesn't exist.
file_age() {
  local m
  if [[ "$OSTYPE" == darwin* ]]; then
    m=$(stat -f %m "$1" 2>/dev/null)
  else
    m=$(stat -c %Y "$1" 2>/dev/null)
  fi
  if [ -n "$m" ]; then echo $(( $(date +%s) - m )); else echo 999999; fi
}

# Color by percentage: green → yellow → orange → red
color_by_pct() {
  local pct=${1:-0} t1=${2:-30} t2=${3:-60} t3=${4:-80}
  if   [ "$pct" -le "$t1" ] 2>/dev/null; then echo "$c_green"
  elif [ "$pct" -le "$t2" ] 2>/dev/null; then echo "$c_yellow"
  elif [ "$pct" -le "$t3" ] 2>/dev/null; then echo "$c_orange"
  else echo "$c_red"
  fi
}

# ═══════════════════════════════════════════════════
# Segments
# ═══════════════════════════════════════════════════

# ── Plan icon ──
seg_plan="${PLAN_ICON}"

# ── Model ──
# Prefer the display name Claude Code already resolves ("Fable 5", "Opus 4.8",
# "Haiku 4.5") — zero maintenance as new families appear. Strip a leading
# "Claude " so long-form names stay tight. Settings-style ids can carry a
# "[1m]" context suffix; drop it before parsing.
model_id="${model_id%%\[*}"
model_label=""
if [ -n "$model_display" ] && [ "$model_display" != "null" ] && [ "$model_display" != "Claude" ]; then
  model_label="${model_display#Claude }"
fi
if [ -z "$model_label" ]; then
  # Fallback for builds without display_name: family from the id, version from
  # the first X-Y digit pair (4-8 → 4.8, and legacy claude-3-7-sonnet → 3.7),
  # else a lone trailing digit group (claude-fable-5 / claude-opus-5 → 5).
  model_name="Claude"; model_version=""
  case "$model_id" in
    *fable*)  model_name="Fable" ;;
    *opus*)   model_name="Opus" ;;
    *sonnet*) model_name="Sonnet" ;;
    *haiku*)  model_name="Haiku" ;;
  esac
  if [[ "$model_id" =~ ([0-9]+)-([0-9]+) ]]; then
    model_version=" ${BASH_REMATCH[1]}.${BASH_REMATCH[2]}"
  elif [[ "$model_id" =~ -([0-9]+)$ ]]; then
    model_version=" ${BASH_REMATCH[1]}"
  fi
  model_label="${model_name}${model_version}"
fi

# Resolve thinking effort. The statusLine stdin exposes the LIVE effort tier at
# .effort.level (read into $effort_level above) — it reflects mid-session
# /effort and /model toggles in real time (low | medium | high | xhigh | max,
# plus "ultra" when ultracode is on) and is absent for models without an
# effort parameter. This is the authoritative source, so we read it directly.
# Fallback: ~/.claude/settings.json effortLevel, only for older Claude Code
# builds that don't emit .effort. Note settings can't hold "max" or "ultra"
# (session-only tiers) — which is exactly why reading .effort.level matters.
if { [ -z "$effort_level" ] || [ "$effort_level" = "null" ]; } && [ -f "$HOME/.claude/settings.json" ]; then
  effort_level=$(jq -r '.effortLevel // empty' "$HOME/.claude/settings.json" 2>/dev/null)
fi

# Display the effort tier truthfully — whatever .effort.level reports
# (low, medium, high, xhigh, max, ultra). No normalization: the tiers are
# distinct, so conflating them would lie about the active setting.
# Uppercased via tr (bash 3.2 on macOS lacks ${var^^}).
effort_display=$(printf '%s' "$effort_level" | tr '[:lower:]' '[:upper:]')
[ "$effort_display" = "NULL" ] && effort_display=""

# Tier colors — muted palette, hotter as the tier climbs; ULTRA sits beyond
# the scale in mauve. Terra cotta stays reserved for the FAST marker so the
# two never blur together. Unknown future tiers render bold-uncolored.
case "$effort_display" in
  LOW)    tier_color="$c_gray" ;;
  MEDIUM) tier_color="$c_white_dim" ;;
  HIGH)   tier_color="$c_green" ;;
  XHIGH)  tier_color="$c_yellow" ;;
  MAX)    tier_color="$c_red" ;;
  ULTRA)  tier_color="$c_magenta" ;;
  *)      tier_color="" ;;
esac
[ -n "$effort_display" ] && effort_colored="${c_bold}${tier_color}${effort_display}${c_reset}${c_dim}" || effort_colored=""

# Build tier suffix: (1M|XHIGH), (1M|MAX), (1M|ULTRA|FAST), (200K), or empty.
# Order: context | effort tier | fast-mode marker. Fast mode (/fast) is a
# separate speed axis from effort — shown only when active so it adds no
# clutter when off. Not bold (per pref: bold is reserved for the effort tier).
tier_inner=""
[ "$context_size" -gt 0 ] 2>/dev/null && tier_inner="$(format_context "$context_size")"
if [ -n "$effort_display" ]; then
  [ -n "$tier_inner" ] && tier_inner="${tier_inner}|${effort_colored}" || tier_inner="${effort_colored}"
fi
if [ "$fast_mode" = "true" ]; then
  fast_marker="${c_orange}FAST${c_reset}${c_dim}"
  [ -n "$tier_inner" ] && tier_inner="${tier_inner}|${fast_marker}" || tier_inner="${fast_marker}"
fi
ctx_tier=""
[ -n "$tier_inner" ] && ctx_tier=" ${c_dim}(${tier_inner})${c_reset}"

seg_model="${c_cyan}${model_label}${c_reset}${ctx_tier}"

# ── Project ──
project_name=""
if [ -n "$project_dir" ] && [ "$project_dir" != "null" ]; then
  project_name=$(basename "$project_dir")
fi
seg_project="${c_yellow}${project_name}${c_reset}"

# ── Git ──
seg_git=""
if [ -n "$project_dir" ] && [ "$project_dir" != "null" ]; then
  git_dir="$project_dir"
  found_git=false
  while [ -n "$git_dir" ] && [ "$git_dir" != "/" ]; do
    [ -d "$git_dir/.git" ] && { found_git=true; break; }
    git_dir=$(dirname "$git_dir")
  done
  if [ "$found_git" = true ]; then
    branch=$(git -C "$git_dir" -c core.useBuiltinFSMonitor=false rev-parse --abbrev-ref HEAD 2>/dev/null)
    if [ -n "$branch" ]; then
      dirty=$(git -C "$git_dir" -c core.useBuiltinFSMonitor=false status --porcelain 2>/dev/null)
      ahead=$(git -C "$git_dir" -c core.useBuiltinFSMonitor=false rev-list --count @{u}..HEAD 2>/dev/null || echo "0")
      behind=$(git -C "$git_dir" -c core.useBuiltinFSMonitor=false rev-list --count HEAD..@{u} 2>/dev/null || echo "0")
      if [ -n "$dirty" ]; then
        seg_git="${c_reset}${branch} ${c_orange}✗${c_reset}"
      else
        seg_git="${c_reset}${branch} ${c_green}✓${c_reset}"
      fi
      [ "${ahead:-0}" -gt 0 ] 2>/dev/null && seg_git+=" ${c_cyan}↑${ahead}${c_reset}"
      [ "${behind:-0}" -gt 0 ] 2>/dev/null && seg_git+=" ${c_magenta}↓${behind}${c_reset}"
    fi
  fi
fi

# ── Tokens ──
# Exact when the build provides current_usage (input + output + cache);
# otherwise derived from size × used% like before.
total_tokens=0
if [ "${ctx_tokens:-0}" -gt 0 ] 2>/dev/null; then
  total_tokens=$ctx_tokens
elif [ "$context_size" -gt 0 ] 2>/dev/null && [ "$used_pct" -gt 0 ] 2>/dev/null; then
  total_tokens=$(( (context_size * used_pct) / 100 ))
fi
tokens_fmt=$(format_tokens "$total_tokens")
size_fmt=$(format_context "$context_size")
token_color=$(color_by_pct "$used_pct" "$TOKEN_THRESH_GREEN" "$TOKEN_THRESH_YELLOW" "$TOKEN_THRESH_ORANGE")
seg_tokens="${token_color}${tokens_fmt}/${size_fmt}${c_reset}"

# ── Per-model weekly buckets (via claude-swap) ──
# The statusLine stdin only carries account-wide 5h/7d; the per-model weekly
# buckets (the "Fable 5" row in /usage) come from `cswap list --json` —
# last-good local data, kept fresh wherever AI smartbar's tray is polling.
# A statusline must never block on a ~0.4s python spawn, so renders read a
# cache file; a refresh runs in the background at most once per SCOPED_TTL.
# First-ever run (no cache yet) refreshes synchronously so the segment works
# even where orphaned background jobs get reaped. No cswap → no segment.
scoped_lines=""
if [ "${STATUSLINE_SCOPED:-on}" != "off" ]; then
  cswap_bin="${SMARTBAR_CSWAP:-}"
  [ -n "$cswap_bin" ] || cswap_bin="$(command -v cswap 2>/dev/null)"
  [ -n "$cswap_bin" ] && [ ! -x "$cswap_bin" ] && cswap_bin=""
  [ -n "$cswap_bin" ] || { [ -x "$HOME/.local/bin/cswap" ] && cswap_bin="$HOME/.local/bin/cswap"; }
  if [ -n "$cswap_bin" ]; then
    scoped_dir="$HOME/.cache/claude-statusline"
    scoped_cache="$scoped_dir/cswap.json"
    mkdir -p "$scoped_dir" 2>/dev/null
    scoped_refresh() {
      local tmp="$scoped_cache.tmp.$$"
      if command -v timeout >/dev/null 2>&1; then
        timeout 15 "$cswap_bin" list --json > "$tmp" 2>/dev/null && mv -f "$tmp" "$scoped_cache" 2>/dev/null
      else
        "$cswap_bin" list --json > "$tmp" 2>/dev/null && mv -f "$tmp" "$scoped_cache" 2>/dev/null
      fi
      rm -f "$tmp" 2>/dev/null
    }
    if [ ! -s "$scoped_cache" ]; then
      scoped_refresh
    elif [ "$(file_age "$scoped_cache")" -gt "$SCOPED_TTL" ] && [ "$(file_age "$scoped_cache.stamp")" -gt "$SCOPED_TTL" ]; then
      touch "$scoped_cache.stamp" 2>/dev/null
      ( scoped_refresh </dev/null >/dev/null 2>&1 & )
    fi
    if [ -s "$scoped_cache" ]; then
      # "Name|pct|reset-epoch" per bucket of the active account. cswap emits
      # UTC ISO timestamps with fractional seconds; normalize for jq's parser
      # and fall back to 0 (no reset shown) on anything unexpected.
      scoped_lines=$(jq -r '
        .accounts[]? | select(.active == true) | .usage.scoped[]? |
        [ (.name // "?"),
          ((.pct // 0) | round),
          ((.resetsAt // "") | sub("\\.[0-9]+"; "") | sub("\\+00:00$"; "Z") | (fromdateiso8601? // 0))
        ] | join("|")' "$scoped_cache" 2>/dev/null)
    fi
  fi
fi

# ── Rate limits (skipped when absent — appears after first API call) ──
rate_parts=()
if [ -n "$five_hour_pct" ]; then
  fh_color=$(color_by_pct "$five_hour_pct" "$RATE_THRESH_GREEN" "$RATE_THRESH_YELLOW" "$RATE_THRESH_ORANGE")
  fh_reset=$(format_reset "$five_hour_reset")
  fh_seg="${fh_color} ${five_hour_pct}%${c_reset}"
  [ -n "$fh_reset" ] && fh_seg+=" ${c_dim}(${fh_reset})${c_reset}"
  rate_parts+=("${c_bold}${c_white_dim}5h${c_reset}${fh_seg}")
fi
if [ -n "$seven_day_pct" ]; then
  sd_color=$(color_by_pct "$seven_day_pct" "$RATE_THRESH_GREEN" "$RATE_THRESH_YELLOW" "$RATE_THRESH_ORANGE")
  sd_reset=$(format_reset "$seven_day_reset")
  sd_seg="${sd_color} ${seven_day_pct}%${c_reset}"
  [ -n "$sd_reset" ] && sd_seg+=" ${c_dim}(${sd_reset})${c_reset}"
  rate_parts+=("${c_bold}${c_white_dim}7d${c_reset}${sd_seg}")
fi
if [ -n "$scoped_lines" ]; then
  while IFS='|' read -r s_name s_pct s_epoch; do
    [ -n "$s_name" ] && [ -n "$s_pct" ] || continue
    # Single-letter label (Fable → F), matching the tight 5h/7d style.
    s_label=$(printf '%s' "$s_name" | cut -c1 | tr '[:lower:]' '[:upper:]')
    s_color=$(color_by_pct "$s_pct" "$RATE_THRESH_GREEN" "$RATE_THRESH_YELLOW" "$RATE_THRESH_ORANGE")
    s_seg="${s_color} ${s_pct}%${c_reset}"
    # Reset shown only when it deviates from the 7d cadence (>1h) — the
    # buckets normally reset together, so repeating the timestamp is noise.
    show_reset=0
    if [ "${s_epoch:-0}" -gt 0 ] 2>/dev/null; then
      if [ -n "$seven_day_reset" ] && [ "$seven_day_reset" != "0" ]; then
        s_diff=$(( s_epoch - seven_day_reset ))
        [ "$s_diff" -lt 0 ] && s_diff=$(( -s_diff ))
        [ "$s_diff" -gt 3600 ] && show_reset=1
      else
        show_reset=1
      fi
    fi
    if [ "$show_reset" = "1" ]; then
      s_reset=$(format_reset "$s_epoch")
      [ -n "$s_reset" ] && s_seg+=" ${c_dim}(${s_reset})${c_reset}"
    fi
    rate_parts+=("${c_bold}${c_white_dim}${s_label}${c_reset}${s_seg}")
  done <<< "$scoped_lines"
fi
seg_rate=""
for part in "${rate_parts[@]}"; do
  [ -n "$seg_rate" ] && seg_rate+="${c_dim}/${c_reset}"
  seg_rate+="$part"
done

# ── Output style ──
seg_style=""
if [ "$output_style" != "default" ] && [ -n "$output_style" ] && [ "$output_style" != "null" ]; then
  seg_style="${c_pink}${output_style}${c_reset}"
fi

# ═══════════════════════════════════════════════════
# Assembly
# ═══════════════════════════════════════════════════
output=""
[ -n "$project_name" ] && output+="${seg_project}${sep}"
[ -n "$seg_git" ]      && output+="${seg_git}${sep}"
output+="${seg_tokens}"
[ -n "$seg_rate" ]     && output+="${sep}${seg_rate}"
output+="${sep}${seg_model}"
[ -n "$seg_style" ]    && output+="${sep}${seg_style}"

printf "%b" "$output"
