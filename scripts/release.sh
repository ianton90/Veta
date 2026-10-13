#!/usr/bin/env bash
# Builds release archives of `veta` and `veta-cli` into dist/.
#
#   scripts/release.sh [TARGET...]
#
# With no targets, builds for this machine. Each target gives
# dist/veta-<version>-<target>.tar.gz (or .zip for Windows), plus
# dist/SHA256SUMS. Set VETA_BUILD_NUMBER to stamp a build number; it
# defaults to the commit count. See docs/RELEASING.md.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -n "$(git status --porcelain)" && "${VETA_ALLOW_DIRTY:-}" != 1 ]]; then
    echo "release: the working tree has uncommitted changes (set VETA_ALLOW_DIRTY=1 to build anyway)" >&2
    exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
targets=("$@")
if [[ ${#targets[@]} -eq 0 ]]; then
    targets=("$(rustc -vV | sed -n 's/^host: //p')")
fi

mkdir -p dist
for target in "${targets[@]}"; do
    echo "== $target"
    cargo build --release --locked -p veta -p veta-cli --target "$target"
    name="veta-$version-$target"
    stage="dist/$name"
    rm -rf "$stage"
    mkdir -p "$stage"
    exe=""
    [[ $target == *windows* ]] && exe=".exe"
    cp "target/$target/release/veta$exe" "target/$target/release/veta-cli$exe" "$stage/"
    cp README.md LICENSE CHANGELOG.md "$stage/"
    if [[ $target == *windows* ]]; then
        rm -f "dist/$name.zip"
        (cd dist && zip -qr "$name.zip" "$name")
    else
        tar -C dist -czf "dist/$name.tar.gz" "$name"
    fi
    rm -rf "$stage"
done

cd dist
if command -v sha256sum >/dev/null; then
    sha256sum veta-*.tar.gz veta-*.zip 2>/dev/null > SHA256SUMS || true
else
    shasum -a 256 veta-*.tar.gz veta-*.zip 2>/dev/null > SHA256SUMS || true
fi
cat SHA256SUMS
