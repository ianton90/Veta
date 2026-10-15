#!/usr/bin/env bash
# Builds release packages of `veta` and `veta-cli` into dist/.
#
#   scripts/release.sh [TARGET...]
#
# With no targets, builds for this machine. Per target:
#   Linux:   veta-<version>-<target>.tar.gz and Veta-<version>-<arch>.AppImage
#            (needs appimagetool: $APPIMAGETOOL, PATH or ~/.cache/veta-tools;
#            $APPIMAGE_RUNTIME may name its runtime file)
#   Windows: veta-<version>-<target>.zip and veta-<version>-windows-<arch>.exe,
#            the installer (needs makensis)
#   macOS:   veta-<version>-<target>.tar.gz and Veta-<version>-<arch>.dmg
#            (on a Mac only)
# plus dist/SHA256SUMS. A missing packaging tool skips that package with a
# warning. Set VETA_BUILD_NUMBER to stamp a build number; it defaults to the
# commit count. See docs/RELEASING.md.
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)

if [[ -n "$(git status --porcelain)" && "${VETA_ALLOW_DIRTY:-}" != 1 ]]; then
    echo "release: the working tree has uncommitted changes (set VETA_ALLOW_DIRTY=1 to build anyway)" >&2
    exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
build=${VETA_BUILD_NUMBER:-$(git rev-list --count HEAD)}
export VETA_BUILD_NUMBER=$build
targets=("$@")
if [[ ${#targets[@]} -eq 0 ]]; then
    targets=("$(rustc -vV | sed -n 's/^host: //p')")
fi

warn() { echo "release: warning: $*" >&2; }

# A path for native Windows tools when running in Git Bash.
native_path() {
    if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi
}

zip_dir() { # <zip> <dir>, from dist/
    if command -v zip >/dev/null; then
        zip -qr "$1" "$2"
    else
        7z a -tzip -bd -bso0 "$1" "$2"
    fi
}

find_appimagetool() {
    if [[ -n "${APPIMAGETOOL:-}" ]]; then
        echo "$APPIMAGETOOL"
    elif command -v appimagetool >/dev/null; then
        command -v appimagetool
    elif [[ -x "$HOME/.cache/veta-tools/appimagetool" ]]; then
        echo "$HOME/.cache/veta-tools/appimagetool"
    fi
}

appimage() { # <bin dir> <arch>
    local tool appdir out
    tool=$(find_appimagetool)
    if [[ -z "$tool" ]]; then
        warn "appimagetool not found, skipping the AppImage"
        return
    fi
    appdir=$(mktemp -d)/Veta.AppDir
    mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
        "$appdir/usr/share/icons/hicolor/256x256/apps" "$appdir/usr/share/mime/packages"
    cp "$1/veta" "$1/veta-cli" "$appdir/usr/bin/"
    cp packaging/linux/AppRun "$appdir/AppRun"
    cp packaging/linux/veta.desktop "$appdir/veta.desktop"
    cp packaging/linux/veta.desktop "$appdir/usr/share/applications/"
    cp packaging/icons/veta-256.png "$appdir/veta.png"
    cp packaging/icons/veta-256.png "$appdir/usr/share/icons/hicolor/256x256/apps/veta.png"
    [[ -f packaging/linux/veta-mime.xml ]] && cp packaging/linux/veta-mime.xml "$appdir/usr/share/mime/packages/veta.xml"
    out="$root/dist/Veta-$version-$2.AppImage"
    # A runtime fetched beforehand ($APPIMAGE_RUNTIME) saves appimagetool
    # downloading it.
    local runtime=()
    [[ -n "${APPIMAGE_RUNTIME:-}" ]] && runtime=(--runtime-file "$APPIMAGE_RUNTIME")
    ARCH=$2 "$tool" --appimage-extract-and-run --no-appstream "${runtime[@]}" "$appdir" "$out" >/dev/null
    rm -rf "$(dirname "$appdir")"
}

installer() { # <bin dir> <arch>
    if ! command -v makensis >/dev/null; then
        warn "makensis not found, skipping the Windows installer"
        return
    fi
    local src
    src=$(mktemp -d)
    cp "$1/veta.exe" "$1/veta-cli.exe" README.md LICENSE CHANGELOG.md "$src/"
    (cd packaging/windows && makensis -V2 -DVERSION="$version" -DBUILD="$build" \
        -DSRC="$(native_path "$src")" \
        -DOUT="$(native_path "$root/dist/veta-$version-windows-$2.exe")" veta.nsi)
    rm -rf "$src"
}

mkdir -p dist
for target in "${targets[@]}"; do
    echo "== $target"
    cargo build --release --locked -p veta -p veta-cli --target "$target"
    bin="target/$target/release"
    arch=${target%%-*}
    name="veta-$version-$target"
    stage="dist/$name"
    rm -rf "$stage"
    mkdir -p "$stage"
    exe=""
    [[ $target == *windows* ]] && exe=".exe"
    cp "$bin/veta$exe" "$bin/veta-cli$exe" "$stage/"
    cp README.md LICENSE CHANGELOG.md "$stage/"
    if [[ $target == *windows* ]]; then
        rm -f "dist/$name.zip"
        (cd dist && zip_dir "$name.zip" "$name")
        installer "$bin" "${arch/x86_64/x64}"
    else
        tar -C dist -czf "dist/$name.tar.gz" "$name"
        case $target in
            *linux*) appimage "$bin" "$arch" ;;
            *apple*)
                if [[ $(uname) == Darwin ]]; then
                    packaging/macos/make-app.sh "$bin" "$version" "$build" "dist/Veta-$version-$arch.dmg"
                else
                    warn "macOS .dmg needs a Mac, skipping it"
                fi
                ;;
        esac
    fi
    rm -rf "$stage"
done

cd dist
files=$(ls veta-* Veta-* 2>/dev/null | grep -v '^SHA256SUMS$' || true)
if command -v sha256sum >/dev/null; then
    sha256sum $files > SHA256SUMS
else
    shasum -a 256 $files > SHA256SUMS
fi
cat SHA256SUMS
