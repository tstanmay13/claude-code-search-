#!/usr/bin/env bash
# Install claude-code-search: build the Rust binary and symlink it into ~/.local/bin.
# A symlink (not a copy) means `git pull && ./install.sh` updates the live command.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="$HOME/.local/bin"

# --- dependency check --------------------------------------------------------
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
command -v cargo >/dev/null || {
  echo "cargo is required: curl https://sh.rustup.rs -sSf | sh" >&2
  exit 1
}
for dep in fzf claude; do
  command -v "$dep" >/dev/null || echo "Missing runtime dependency: $dep (brew install fzf; claude: https://claude.com/claude-code)" >&2
done

# --- build and link -----------------------------------------------------------
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"
mkdir -p "$TARGET_DIR"
ln -sfn "$REPO_DIR/target/release/ccs" "$TARGET_DIR/ccs"
echo "linked: $TARGET_DIR/ccs -> $REPO_DIR/target/release/ccs"

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
echo "Done. Try: ccs <query>"
