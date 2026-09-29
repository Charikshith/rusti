#!/bin/sh
# rusti installer for Linux and macOS:
#   curl -fsSL https://raw.githubusercontent.com/Charikshith/rusti/master/install.sh | sh
# Puts rusti in ~/.rusti/bin and adds that folder to your shell's PATH.
# Run it again to update.
set -e
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) asset=rusti-linux-x86_64 ;;
  Darwin-arm64) asset=rusti-macos-aarch64 ;;
  Darwin-x86_64) asset=rusti-macos-x86_64 ;;
  *) echo "no prebuilt rusti for $(uname -s) $(uname -m); build from source: cargo install --git https://github.com/Charikshith/rusti" >&2; exit 1 ;;
esac
bin="$HOME/.rusti/bin"
url="https://github.com/Charikshith/rusti/releases/latest/download/$asset"
mkdir -p "$bin"
echo "downloading $url"
# download beside the old binary, then rename: replacing a running file in place fails
curl -fsSL "$url" -o "$bin/rusti.new"
chmod +x "$bin/rusti.new"
mv "$bin/rusti.new" "$bin/rusti"

case ":$PATH:" in
  *":$bin:"*) ;;
  *)
    case "$SHELL" in
      */zsh) rc="$HOME/.zshrc" ;;
      */bash) rc="$HOME/.bashrc" ;;
      *) rc="$HOME/.profile" ;;
    esac
    if ! grep -qs '.rusti/bin' "$rc"; then
      echo 'export PATH="$HOME/.rusti/bin:$PATH"' >> "$rc"
      echo "added $bin to PATH in $rc (open a new terminal to pick it up)"
    fi
    ;;
esac
echo "installed $("$bin/rusti" --version) -> $bin/rusti"
echo "run: rusti --tui"
