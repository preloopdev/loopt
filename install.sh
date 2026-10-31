#!/bin/sh
# Install a prebuilt Loopt binary from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/preloopdev/loopt/main/install.sh | sh
#
# Environment:
#   LOOPT_VERSION       Release tag to install (default: latest)
#   LOOPT_INSTALL_DIR   Destination directory (default: ~/.local/bin)
#   LOOPT_REPO          GitHub repository (default: preloopdev/loopt)
#
# No Cargo, Rust toolchain, or root access is required.

set -eu

REPO="${LOOPT_REPO:-preloopdev/loopt}"
VERSION="${LOOPT_VERSION:-latest}"
INSTALL_DIR="${LOOPT_INSTALL_DIR:-$HOME/.local/bin}"

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'loopt install: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

need curl
need tar
need uname
need mktemp

case "$(uname -s)" in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) die "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
    arm64|aarch64)
        if [ "$os" = darwin ]; then arch=arm64; else arch=aarch64; fi
        ;;
    x86_64|amd64) arch=x86_64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
esac

if [ "$VERSION" = latest ]; then
    VERSION="$(
        curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
            | head -n 1
    )"
    [ -n "$VERSION" ] || die "could not resolve the latest release for $REPO"
fi

asset="loopt-${VERSION}-${os}-${arch}.tar.gz"
url="https://github.com/$REPO/releases/download/$VERSION/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

say "Downloading $asset"
curl -fL --retry 3 --proto '=https' --tlsv1.2 -o "$tmp/$asset" "$url" \
    || die "no prebuilt binary for ${os}/${arch} in $VERSION (looked for $asset)"

if curl -fsL --proto '=https' --tlsv1.2 -o "$tmp/$asset.sha256" "$url.sha256" 2>/dev/null; then
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$tmp" && sha256sum -c "$asset.sha256")
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$tmp" && shasum -a 256 -c "$asset.sha256")
    else
        die "checksum file downloaded but no sha256 tool is available"
    fi
else
    say "Warning: no checksum published for $asset; continuing with HTTPS only." >&2
fi

tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/loopt" ] || die "archive did not contain a loopt binary"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/loopt" "$INSTALL_DIR/loopt"

say "Installed $("$INSTALL_DIR/loopt" --version) to $INSTALL_DIR/loopt"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) say "Add $INSTALL_DIR to PATH to run loopt from any directory." ;;
esac
