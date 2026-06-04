#!/bin/sh
# Sigo installer: downloads a prebuilt binary from GitHub Releases.
# Usage: curl -fsSL https://raw.githubusercontent.com/twigglits/Sigo/main/install.sh | sh
set -eu

REPO="twigglits/Sigo"
BIN="sigo"
: "${SIGO_VERSION:=latest}"

err() {
  echo "error: $*" >&2
  exit 1
}
info() { echo ">> $*"; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) err "unsupported OS: $os (use Docker or build from source)" ;;
esac
case "$arch" in
  x86_64 | amd64) arch_part="x86_64" ;;
  arm64 | aarch64) arch_part="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"

if [ "$SIGO_VERSION" = "latest" ]; then
  tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" |
    grep '"tag_name"' | head -n1 | cut -d'"' -f4)"
  [ -n "$tag" ] || err "could not resolve latest release tag (set SIGO_VERSION=vX.Y.Z)"
else
  tag="$SIGO_VERSION"
fi
info "installing ${BIN} ${tag} for ${target}"

asset="sigo-${target}.tar.gz"
base="https://github.com/${REPO}/releases/download/${tag}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading ${asset}"
curl -fsSL "${base}/${asset}" -o "${tmp}/${asset}" || err "download failed: ${base}/${asset}"
curl -fsSL "${base}/${asset}.sha256" -o "${tmp}/${asset}.sha256" || err "checksum download failed"

info "verifying checksum"
(
  cd "$tmp"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${asset}.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "${asset}.sha256"
  else
    err "no sha256 tool (sha256sum / shasum) found"
  fi
)

tar -xzf "${tmp}/${asset}" -C "$tmp"
[ -f "${tmp}/${BIN}" ] || err "archive did not contain ${BIN}"
chmod +x "${tmp}/${BIN}"

dest="$HOME/.local/bin"
if mkdir -p "$dest" 2>/dev/null && [ -w "$dest" ]; then
  mv "${tmp}/${BIN}" "${dest}/${BIN}"
else
  dest="/usr/local/bin"
  info "installing to ${dest} (requires sudo)"
  sudo mv "${tmp}/${BIN}" "${dest}/${BIN}"
fi
info "installed ${dest}/${BIN}"

case ":$PATH:" in
  *":$dest:"*) ;;
  *) info "NOTE: ${dest} is not on your PATH — add it to your shell profile" ;;
esac

cat <<EOF

Next steps:
  1. Install Ollama:        https://ollama.com
  2. Pull the model:        ollama pull qwen2.5:7b
  3. Set your Claude key:   export ANTHROPIC_API_KEY=sk-...
  4. Verify setup:          ${BIN} doctor
  5. Start chatting:        ${BIN}
EOF
