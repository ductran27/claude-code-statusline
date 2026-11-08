#!/bin/sh
# Install claude-statusline into ~/.claude/bin (macOS, Linux, Git Bash on Windows).
# Builds from this checkout when cargo is available, otherwise downloads the
# latest release binary (CLAUDE_STATUSLINE_DOWNLOAD=1 forces the download).
# The old binary is replaced by rename, so Claude Code sessions that are
# running it keep working.
set -eu

repo_dir=$(cd "$(dirname "$0")" && pwd)
dest=${CLAUDE_STATUSLINE_DIR:-"$HOME/.claude/bin"}
releases=https://github.com/ductran27/claude-code-statusline/releases/latest/download

os=$(uname -s)
arch=$(uname -m)
exe=claude-statusline
case "$os" in
  MINGW* | MSYS* | CYGWIN*) exe=claude-statusline.exe ;;
esac

cargo_bin=$(command -v cargo 2>/dev/null || true)
[ -n "$cargo_bin" ] || { [ -x "$HOME/.cargo/bin/cargo" ] && cargo_bin="$HOME/.cargo/bin/cargo"; } || true

if [ -n "$cargo_bin" ] && [ -f "$repo_dir/Cargo.toml" ] && [ "${CLAUDE_STATUSLINE_DOWNLOAD:-}" != 1 ]; then
  echo "Building with $cargo_bin"
  "$cargo_bin" build --release --locked --manifest-path "$repo_dir/Cargo.toml"
  src="$repo_dir/target/release/$exe"
else
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) echo "No prebuilt binary for $arch; install Rust (https://rustup.rs) and rerun." >&2; exit 1 ;;
  esac
  case "$os" in
    Darwin) target="$arch-apple-darwin" ;;
    Linux) target="$arch-unknown-linux-musl" ;;
    MINGW* | MSYS* | CYGWIN*) target="x86_64-pc-windows-msvc.exe" ;;
    *) echo "No prebuilt binary for $os; install Rust (https://rustup.rs) and rerun." >&2; exit 1 ;;
  esac
  src="${TMPDIR:-/tmp}/claude-statusline.$$"
  echo "Downloading claude-statusline-$target"
  curl -fsSL -o "$src" "$releases/claude-statusline-$target"
fi

mkdir -p "$dest"
tmp="$dest/.$exe.new.$$"
cp "$src" "$tmp"
chmod 755 "$tmp"
# Windows cannot overwrite a running .exe but can rename it.
case "$exe" in *.exe) [ ! -e "$dest/$exe" ] || mv -f "$dest/$exe" "$dest/$exe.old.$$" ;; esac
mv -f "$tmp" "$dest/$exe"
rm -f "$dest/$exe".old.* 2>/dev/null || true
case "$src" in "${TMPDIR:-/tmp}"/claude-statusline.*) rm -f "$src" ;; esac

# Leftovers of the earlier shell version.
old_cache="$HOME/.cache/claude-statusline"
rm -f "$old_cache"/cswap.json "$old_cache"/cswap.json.stamp "$old_cache"/cswap.json.tmp.* 2>/dev/null || true

case "$exe" in
  *.exe) cmd=$(cd "$dest" && pwd -W 2>/dev/null || pwd)/$exe ;;
  *) cmd="~/.claude/bin/$exe"; [ "$dest" = "$HOME/.claude/bin" ] || cmd="$dest/$exe" ;;
esac
echo "Installed $dest/$exe ($("$dest/$exe" </dev/null >/dev/null 2>&1 && echo ok))"
cat <<EOF

Point Claude Code at it in ~/.claude/settings.json:

  "statusLine": {
    "type": "command",
    "command": "$cmd",
    "refreshInterval": 60
  }
EOF
