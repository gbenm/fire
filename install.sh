#!/bin/sh
# Install the latest (or a given) Fire release from GitHub.
#
#   curl -fsSL https://raw.githubusercontent.com/gbenm/fire/main/install.sh | sh
#
# Environment:
#   FIRE_VERSION      tag to install, e.g. v2.10.0 (default: latest)
#   FIRE_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
set -eu

REPO="gbenm/fire"
INSTALL_DIR="${FIRE_INSTALL_DIR:-$HOME/.local/bin}"

err() { echo "fire-install: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || err "missing required command: $1"; }

need curl
need tar

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)          suffix="macos-arm64" ;;
  Darwin/x86_64)         suffix="macos-intel" ;;
  Linux/x86_64|Linux/amd64)   suffix="linux-x86_64" ;;
  Linux/aarch64|Linux/arm64)  suffix="linux-arm64" ;;
  *) err "no prebuilt binary for $os/$arch; build from source with: cargo install --git https://github.com/$REPO" ;;
esac

version="${FIRE_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$version" ] || err "could not determine the latest release"
fi

archive="fire-${version}-${suffix}.tar.gz"
base="https://github.com/$REPO/releases/download/$version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading fire $version ($suffix)..."
curl -fsSL "$base/$archive" -o "$tmp/$archive" || err "download failed: $base/$archive"

if curl -fsSL "$base/checksums.txt" -o "$tmp/checksums.txt" 2>/dev/null; then
  expected="$(grep " $archive\$" "$tmp/checksums.txt" | awk '{print $1}')"
  if [ -n "$expected" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
    else
      actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
    fi
    [ "$expected" = "$actual" ] || err "checksum mismatch for $archive"
  fi
fi

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$INSTALL_DIR"
mv "$tmp/fire" "$INSTALL_DIR/fire"
chmod +x "$INSTALL_DIR/fire"

echo "Installed fire $version to $INSTALL_DIR/fire"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Note: $INSTALL_DIR is not on your PATH. Add it to your shell profile." ;;
esac
echo "Next: run 'fire cli init' in a project, then 'fire cli completion install'."
