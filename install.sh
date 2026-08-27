#!/bin/sh
# ApiTest installer: downloads the latest GitHub release for this machine,
# verifies its checksum and installs the binary into ~/.local/bin.
#
#   curl -fsSL https://raw.githubusercontent.com/zzhtl/apitest/main/install.sh | sh
#
# Environment overrides:
#   APITEST_VERSION      install a specific tag (e.g. v0.1.0) instead of latest
#   APITEST_INSTALL_DIR  target directory (default: ~/.local/bin)
set -eu

REPO="zzhtl/apitest"
BINARY="apitest"
INSTALL_DIR="${APITEST_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux)
        case "$arch" in
            x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
            aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
            *) fail "unsupported Linux architecture: $arch (download manually: https://github.com/$REPO/releases)" ;;
        esac
        ;;
    Darwin)
        case "$arch" in
            x86_64) target="x86_64-apple-darwin" ;;
            arm64) target="aarch64-apple-darwin" ;;
            *) fail "unsupported macOS architecture: $arch (download manually: https://github.com/$REPO/releases)" ;;
        esac
        ;;
    *)
        fail "unsupported platform: $os. On Windows, download the .zip from https://github.com/$REPO/releases"
        ;;
esac

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    latest_tag() {
        curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" |
            sed 's#.*/tag/##'
    }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q "$1" -O "$2"; }
    latest_tag() {
        wget -q -S --max-redirect=0 "https://github.com/$REPO/releases/latest" -O /dev/null 2>&1 |
            awk '/Location:/ { print $2 }' | tail -1 | sed 's#.*/tag/##'
    }
else
    fail "neither curl nor wget is available"
fi

tag="${APITEST_VERSION:-$(latest_tag)}"
case "$tag" in
    v*) ;;
    "") fail "could not resolve the latest release tag" ;;
    *) tag="v$tag" ;;
esac
version="${tag#v}"
archive="$BINARY-v$version-$target.tar.gz"
base_url="https://github.com/$REPO/releases/download/$tag"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

say "Downloading $archive ($tag)..."
fetch "$base_url/$archive" "$workdir/$archive"
fetch "$base_url/SHA256SUMS" "$workdir/SHA256SUMS"

say "Verifying checksum..."
(
    cd "$workdir"
    expected="$(grep " $archive\$" SHA256SUMS || true)"
    [ -n "$expected" ] || fail "SHA256SUMS has no entry for $archive"
    if command -v sha256sum >/dev/null 2>&1; then
        printf '%s\n' "$expected" | sha256sum -c - >/dev/null
    elif command -v shasum >/dev/null 2>&1; then
        printf '%s\n' "$expected" | shasum -a 256 -c - >/dev/null
    else
        fail "neither sha256sum nor shasum is available to verify the download"
    fi
)

say "Installing to $INSTALL_DIR..."
mkdir -p "$INSTALL_DIR"
tar -C "$workdir" -xzf "$workdir/$archive" "./$BINARY" 2>/dev/null ||
    tar -C "$workdir" -xzf "$workdir/$archive" "$BINARY"
install -m 755 "$workdir/$BINARY" "$INSTALL_DIR/$BINARY"

say ""
say "ApiTest $tag installed: $INSTALL_DIR/$BINARY"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        say ""
        say "note: $INSTALL_DIR is not on your PATH. Add this to your shell profile:"
        say "  export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac

case "$os" in
    Linux)
        say ""
        say "Runtime requirements on Linux:"
        say "  - a Vulkan-capable GPU driver (e.g. mesa-vulkan-drivers)"
        say "  - libxkbcommon (usually preinstalled on desktops)"
        say "  - a Secret Service daemon for stored secrets (gnome-keyring or KWallet)"
        say "  - a Simplified-Chinese font for the Chinese UI (e.g. fonts-noto-cjk)"
        ;;
    Darwin)
        say ""
        say "macOS ships unsigned builds; if Gatekeeper blocks the first launch run:"
        say "  xattr -d com.apple.quarantine \"$INSTALL_DIR/$BINARY\""
        ;;
esac
