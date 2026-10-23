#!/bin/sh
set -e

REPO="${SEEKFETCH_REPO:-Halo94361/seekfetch-rs}"
PREFIX="${PREFIX:-${HOME}/.local/bin}"
BIN="seekfetch"

os=$(uname -s)
arch=$(uname -m)

case "${os}-${arch}" in
  Linux-x86_64) target="linux-x86_64" ;;
  *)
    echo "unsupported platform: ${os} ${arch}" >&2
    echo "download a release asset instead: https://github.com/${REPO}/releases/latest" >&2
    exit 1
    ;;
esac

tag=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep -o '"tag_name":[[:space:]]*"[^"]*"' \
  | head -n 1 \
  | cut -d'"' -f4)

if [ -z "${tag}" ]; then
  echo "could not resolve the latest release tag for ${REPO}" >&2
  exit 1
fi

url="https://github.com/${REPO}/releases/download/${tag}/seekfetch-${tag}-${target}"
tmp="${PREFIX}/.${BIN}.$$"

mkdir -p "${PREFIX}"
curl -fsSL "${url}" -o "${tmp}"
chmod +x "${tmp}"
mv "${tmp}" "${PREFIX}/${BIN}"

echo "installed ${PREFIX}/${BIN} (${tag})"
