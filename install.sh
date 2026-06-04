#!/usr/bin/env bash
# Install cc-tools: symlink every script in bin/ into ~/.local/bin.
# Symlinks (not copies) so a `git pull` in this repo updates the live commands.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="$HOME/.local/bin"

# --- dependency check --------------------------------------------------------
missing=()
for dep in rg fzf jq claude; do
  command -v "$dep" >/dev/null || missing+=("$dep")
done
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "Missing dependencies: ${missing[*]}" >&2
  echo "  brew install ripgrep fzf jq" >&2
  echo "  claude: https://claude.com/claude-code" >&2
  # Don't hard-fail — still install, tools will error helpfully at runtime
fi

# --- symlink scripts ----------------------------------------------------------
mkdir -p "$TARGET_DIR"
for script in "$REPO_DIR"/bin/*; do
  name="$(basename "$script")"
  chmod +x "$script"
  ln -sfn "$script" "$TARGET_DIR/$name"
  echo "linked: $TARGET_DIR/$name -> $script"
done

# --- PATH check ---------------------------------------------------------------
case ":$PATH:" in
  *":$TARGET_DIR:"*) ;;
  *)
    echo ""
    echo "NOTE: $TARGET_DIR is not in your PATH. Add this to your ~/.zshrc:"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    ;;
esac

echo ""
echo "Done. Try: ccs <pattern>"
