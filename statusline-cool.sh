#!/bin/bash

# Statusline for Claude Code — clean, information-dense, dynamically colored
# Format: Icon | Project | Git | Tokens | Rate Limits | Model | Style
#
# Percentages are color-coded (green → yellow → orange → red) to carry the
# threshold signal. Example rate limits segment:
#   5h 10% (2h36m) / 7d 2% (Thu 5PM)

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

IFS='|' read -r model_id context_size used_pct project_dir output_style \
  five_hour_pct five_hour_reset seven_day_pct seven_day_reset effort_level fast_mode \
  <<< "$(echo "$input" | jq -r '[
    (.model.id // "unknown"),
    (.context_window.context_window_size // 0),
    (.context_window.used_percentage // 0),
    (.workspace.project_dir // ""),
    (.output_style.name // "default"),
    (if .rate_limits.five_hour.used_percentage != null then (.rate_limits.five_hour.used_percentage | ceil) else "" end),
    (.rate_limits.five_hour.resets_at // ""),
    (if .rate_limits.seven_day.used_percentage != null then (.rate_limits.seven_day.used_percentage | ceil) else "" end),
    (.rate_limits.seven_day.resets_at // ""),
    (.effort.level // ""),
    (.fast_mode // false)
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
model_name="Claude"; model_version=""
case "$model_id" in
  *opus*)   model_name="Opus" ;;
  *sonnet*) model_name="Sonnet" ;;
  *haiku*)  model_name="Haiku" ;;
esac
if [[ "$model_id" =~ ([0-9]+)-([0-9]+) ]]; then
  model_version=" ${BASH_REMATCH[1]}.${BASH_REMATCH[2]}"
fi

# Resolve thinking effort. The statusLine stdin exposes the LIVE effort tier at
# .effort.level (read into $effort_level above) — it reflects mid-session
# /effort and /model toggles in real time (Opus 4.8: low | medium | high |
# xhigh | max; "ultra" = ultracode) and is absent for models without an effort
# parameter. This is the authoritative source, so we read it directly instead
# of scraping the transcript (the old grep silently missed live toggles).
# Fallback: ~/.claude/settings.json effortLevel, only for older Claude Code
# builds that don't emit .effort. Note settings can't hold "max" (session-only
# tier) — which is exactly why the stale-settings path could never surface a
# live MAX. Reading .effort.level fixes that.
if { [ -z "$effort_level" ] || [ "$effort_level" = "null" ]; } && [ -f "$HOME/.claude/settings.json" ]; then
  effort_level=$(jq -r '.effortLevel // empty' "$HOME/.claude/settings.json" 2>/dev/null)
fi

# Display the effort tier truthfully — whatever .effort.level reports
# (low, medium, high, xhigh, max, ultra). No normalization: xhigh and max are
# distinct tiers, so conflating them would lie about the active setting.
# Uppercased via tr (bash 3.2 on macOS lacks ${var^^}).
effort_display=$(printf '%s' "$effort_level" | tr '[:lower:]' '[:upper:]')
[ "$effort_display" = "NULL" ] && effort_display=""
[ -n "$effort_display" ] && effort_colored="${c_bold}${effort_display}${c_reset}${c_dim}" || effort_colored=""

# Build tier suffix: (1M|XHIGH), (1M|MAX), (1M|MAX|FAST), (200K), or empty.
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

seg_model="${c_cyan}${model_name}${model_version}${c_reset}${ctx_tier}"

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
total_tokens=0
[ "$context_size" -gt 0 ] 2>/dev/null && [ "$used_pct" -gt 0 ] 2>/dev/null && \
  total_tokens=$(( (context_size * used_pct) / 100 ))
tokens_fmt=$(format_tokens "$total_tokens")
size_fmt=$(format_context "$context_size")
token_color=$(color_by_pct "$used_pct" "$TOKEN_THRESH_GREEN" "$TOKEN_THRESH_YELLOW" "$TOKEN_THRESH_ORANGE")
seg_tokens="${token_color}${tokens_fmt}/${size_fmt}${c_reset}"

# ── Rate limits (skipped when absent — appears after first API call) ──
seg_rate=""
if [ -n "$five_hour_pct" ] || [ -n "$seven_day_pct" ]; then
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
  if [ ${#rate_parts[@]} -eq 2 ]; then
    seg_rate="${rate_parts[0]}${c_dim}/${c_reset}${rate_parts[1]}"
  else
    seg_rate="${rate_parts[0]}"
  fi
fi

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
