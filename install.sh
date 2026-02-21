#!/usr/bin/env bash
set -euo pipefail

OWNER="${OWNER:-David-Factor}"
REPO="${REPO:-whence}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

require_cmd curl
require_cmd tar
require_cmd uname
require_cmd mktemp
require_cmd install

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux) target_os="unknown-linux-musl" ;;
  Darwin) target_os="apple-darwin" ;;
  *)
    echo "unsupported OS: $os" >&2
    exit 1
    ;;
esac

case "$arch" in
  x86_64|amd64) target_arch="x86_64" ;;
  arm64|aarch64) target_arch="aarch64" ;;
  *)
    echo "unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  VERSION="$(
    curl -fsSL "https://api.github.com/repos/$OWNER/$REPO/releases/latest" \
      | sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
      | head -n1
  )"
fi

if [ -z "$VERSION" ]; then
  echo "unable to resolve release version" >&2
  exit 1
fi

asset="thence-${VERSION}-${target_arch}-${target_os}.tar.gz"
url="https://github.com/$OWNER/$REPO/releases/download/${VERSION}/${asset}"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "Installing $REPO ${VERSION} for ${target_arch}-${target_os}..."
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  gh release download "$VERSION" \
    --repo "$OWNER/$REPO" \
    --pattern "$asset" \
    --dir "$tmpdir" \
    --clobber >/dev/null
else
  curl -fL "$url" -o "$tmpdir/$asset"
fi
tar -xzf "$tmpdir/$asset" -C "$tmpdir"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmpdir/thence" "$INSTALL_DIR/thence"

echo "Installed: $INSTALL_DIR/thence"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "Add to PATH if needed:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
