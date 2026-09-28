#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
TARGET="x86_64-unknown-linux-gnu"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "required tool '$1' is unavailable (environment blocker)"; }

need cargo
need python3
need tar
need gzip
need sha256sum
need file
[ -n "${SOURCE_DATE_EPOCH:-}" ] || die 'SOURCE_DATE_EPOCH is required for reproducible output (environment blocker)'
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || die 'SOURCE_DATE_EPOCH must be a non-negative integer'
need appimagetool
[ -f "$REPO_ROOT/assets/pyxross.png" ] || die 'missing required AppImage icon assets/pyxross.png'

METADATA="$(cargo metadata --locked --no-deps --format-version 1 --manifest-path "$REPO_ROOT/Cargo.toml")" \
  || die 'compatible Rust toolchain or locked metadata is unavailable (environment blocker)'
read -r PACKAGE VERSION <<EOF
$(printf '%s' "$METADATA" | python3 -c 'import json, sys; p=json.load(sys.stdin)["packages"][0]; print(p["name"], p["version"])')
EOF
[ "$PACKAGE" = pyxross ] || die "Cargo package is '$PACKAGE', expected pyxross"

cargo build --locked --release --target "$TARGET" --manifest-path "$REPO_ROOT/Cargo.toml" \
  || die "Linux $TARGET release build failed (toolchain/build blocker)"
BINARY="$REPO_ROOT/target/$TARGET/release/$PACKAGE"
[ -x "$BINARY" ] || die "built binary not found at $BINARY (script/build error)"

SOURCE_THEME="$REPO_ROOT/assets/themes/default/theme.json"
SOURCE_ATLAS="$REPO_ROOT/assets/themes/default/atlas.png"
[ -f "$SOURCE_THEME" ] || die "missing required resource $SOURCE_THEME"
[ -f "$SOURCE_ATLAS" ] || die "missing required resource $SOURCE_ATLAS"
[ -f "$REPO_ROOT/LICENSE" ] || die 'missing LICENSE'
[ -f "$REPO_ROOT/packaging/package-README.md" ] || die 'missing package README'
[ -f "$REPO_ROOT/packaging/pyxross.desktop" ] || die 'missing desktop entry'

PACKAGE_ROOT="$DIST_DIR/${PACKAGE}-${VERSION}-linux-x86_64"
APPDIR="$DIST_DIR/${PACKAGE}-${VERSION}-linux-x86_64.AppDir"
ARCHIVE="$DIST_DIR/${PACKAGE}-${VERSION}-linux-x86_64.tar.gz"
APPIMAGE="$DIST_DIR/${PACKAGE}-${VERSION}-linux-x86_64.AppImage"
mkdir -p "$DIST_DIR"
rm -rf "$PACKAGE_ROOT" "$APPDIR"
mkdir -p "$PACKAGE_ROOT/themes/builtin" "$APPDIR/usr/bin" "$APPDIR/usr/share/$PACKAGE/themes/builtin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/64x64/apps" "$APPDIR/assets"
cp "$BINARY" "$PACKAGE_ROOT/$PACKAGE"
cp "$SOURCE_THEME" "$PACKAGE_ROOT/themes/builtin/theme.json"
cp "$SOURCE_ATLAS" "$PACKAGE_ROOT/themes/builtin/atlas.png"
cp "$REPO_ROOT/packaging/package-README.md" "$PACKAGE_ROOT/README.md"
cp "$REPO_ROOT/LICENSE" "$PACKAGE_ROOT/LICENSE"
cp "$BINARY" "$APPDIR/usr/bin/$PACKAGE"
cp "$SOURCE_THEME" "$APPDIR/usr/share/$PACKAGE/themes/builtin/theme.json"
cp "$SOURCE_ATLAS" "$APPDIR/usr/share/$PACKAGE/themes/builtin/atlas.png"
cp "$REPO_ROOT/packaging/package-README.md" "$APPDIR/usr/share/$PACKAGE/README.md"
cp "$REPO_ROOT/LICENSE" "$APPDIR/usr/share/$PACKAGE/LICENSE"
cp "$REPO_ROOT/packaging/pyxross.desktop" "$APPDIR/pyxross.desktop"
cp "$REPO_ROOT/assets/pyxross.png" "$APPDIR/assets/pyxross.png"
cp "$REPO_ROOT/assets/pyxross.png" "$APPDIR/pyxross.png"

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/usr/bin/env sh
set -eu
HERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "$HERE/usr/share/pyxross"
exec "$HERE/usr/bin/pyxross" "$@"
APPRUN
chmod 0755 "$APPDIR/AppRun" "$PACKAGE_ROOT/$PACKAGE" "$APPDIR/usr/bin/$PACKAGE"

tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner -cf - -C "$DIST_DIR" "$(basename "$PACKAGE_ROOT")" | gzip -n > "$ARCHIVE"
SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" appimagetool "$APPDIR" "$APPIMAGE" \
  || die 'appimagetool failed; no trustworthy AppImage artifact was accepted'
sha256sum "$ARCHIVE" "$APPIMAGE" > "$DIST_DIR/SHA256SUMS"

tar -tzf "$ARCHIVE" | grep -Fx "$(basename "$PACKAGE_ROOT")/$PACKAGE" >/dev/null || die 'tar archive content check failed'
tar -tzf "$ARCHIVE" | grep -Fx "$(basename "$PACKAGE_ROOT")/themes/builtin/theme.json" >/dev/null || die 'tar theme check failed'
file "$PACKAGE_ROOT/$PACKAGE" | grep -E 'ELF .*executable' >/dev/null || die 'built file is not an ELF executable'
if command -v readelf >/dev/null 2>&1; then readelf -h "$PACKAGE_ROOT/$PACKAGE" | grep -F 'X86-64' >/dev/null || die 'ELF architecture check failed'; fi
rm -rf "$PACKAGE_ROOT" "$APPDIR"
