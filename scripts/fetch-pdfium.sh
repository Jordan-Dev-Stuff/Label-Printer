#!/usr/bin/env bash
# Downloads the pinned pdfium build for this platform into vendor/pdfium/,
# where crates/thermal-ui/build.rs embeds it into the executable. Run once per
# build machine; end users never need it.
#
#   ./scripts/fetch-pdfium.sh
#
# Source: https://github.com/bblanchon/pdfium-binaries (BSD-3).
# The release must match the `pdfium_XXXX` feature of pdfium-render in
# crates/thermal-ui/Cargo.toml; a different build can be missing functions
# the bindings need.
set -euo pipefail

release="chromium/7543"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) asset="pdfium-mac-arm64.tgz"; dir="macos-aarch64"; libname="libpdfium.dylib" ;;
    Darwin-*) asset="pdfium-mac-x64.tgz"; dir="macos-x86_64"; libname="libpdfium.dylib" ;;
    Linux-aarch64) asset="pdfium-linux-arm64.tgz"; dir="linux-aarch64"; libname="libpdfium.so" ;;
    Linux-*) asset="pdfium-linux-x64.tgz"; dir="linux-x86_64"; libname="libpdfium.so" ;;
    *)
        echo "unsupported platform: $(uname -s) $(uname -m)" >&2
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

dest="${root}/vendor/pdfium/${dir}"
mkdir -p "${dest}"
cp "${library}" "${dest}/${libname}"
cp "${work}/LICENSE" "${dest}/LICENSE"
echo "pdfium ${release} -> ${dest}/${libname}"
echo "Build with: cargo build --release"
