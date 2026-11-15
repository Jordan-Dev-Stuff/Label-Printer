#!/usr/bin/env bash
# Downloads the pinned pdfium build into vendor/pdfium/<platform>/, where
# crates/label-printer/build.rs embeds it into the executable. Run once per
# build machine; end users never need it.
#
#   ./scripts/fetch-pdfium.sh               # this machine's platform
#   ./scripts/fetch-pdfium.sh macos-x86_64  # another one, for cross builds
#
# Platforms: linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64. The name
# is `<target_os>-<target_arch>`, the folder build.rs looks in.
#
# Source: https://github.com/bblanchon/pdfium-binaries (BSD-3).
# The release must match the `pdfium_XXXX` feature of pdfium-render in
# crates/label-printer/Cargo.toml; a different build can be missing functions
# the bindings need.
set -euo pipefail

release="chromium/7543"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

platform="${1:-}"
if [ -z "${platform}" ]; then
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64) platform="macos-aarch64" ;;
        Darwin-*) platform="macos-x86_64" ;;
        Linux-aarch64) platform="linux-aarch64" ;;
        Linux-*) platform="linux-x86_64" ;;
        *)
            echo "unsupported platform: $(uname -s) $(uname -m)" >&2
            exit 1
            ;;
    esac
fi

case "${platform}" in
    macos-aarch64) asset="pdfium-mac-arm64.tgz"; libname="libpdfium.dylib" ;;
    macos-x86_64) asset="pdfium-mac-x64.tgz"; libname="libpdfium.dylib" ;;
    linux-aarch64) asset="pdfium-linux-arm64.tgz"; libname="libpdfium.so" ;;
    linux-x86_64) asset="pdfium-linux-x64.tgz"; libname="libpdfium.so" ;;
    *)
        echo "unknown platform ${platform}; use linux-x86_64, linux-aarch64, macos-x86_64 or macos-aarch64" >&2
        exit 1
        ;;
esac

url="https://github.com/bblanchon/pdfium-binaries/releases/download/${release/\//%2F}/${asset}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "Downloading ${asset} (${release})"
if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "${work}/${asset}" "$url"
else
    wget -q -O "${work}/${asset}" "$url"
fi

tar -xzf "${work}/${asset}" -C "${work}"
library="$(find "${work}" -type f -name "${libname}" | head -n 1)"
if [ -z "${library}" ]; then
    echo "no ${libname} inside ${asset}" >&2
    exit 1
fi

dest="${root}/vendor/pdfium/${platform}"
mkdir -p "${dest}"
cp "${library}" "${dest}/${libname}"
cp "${work}/LICENSE" "${dest}/LICENSE"
echo "pdfium ${release} -> ${dest}/${libname}"
