#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
need() { command -v "$1" >/dev/null 2>&1 || { printf 'BLOCKED: %s unavailable\n' "$1"; exit 2; }; }
need tar; need sha256sum; need file; need find; need awk; need grep; need mktemp; need sort
mapfile -t archives < <(find "$DIST_DIR" -maxdepth 1 -type f -name 'pyxross-*-linux-x86_64.tar.gz' -print 2>/dev/null | sort)
[ "${#archives[@]}" -eq 1 ] || { printf 'BLOCKED: expected exactly one Linux tar artifact in %s\n' "$DIST_DIR"; exit 2; }
mapfile -t appimages < <(find "$DIST_DIR" -maxdepth 1 -type f -name 'pyxross-*-linux-x86_64.AppImage' -print 2>/dev/null | sort)
[ "${#appimages[@]}" -eq 1 ] || { printf 'BLOCKED: expected exactly one Linux AppImage artifact in %s\n' "$DIST_DIR"; exit 2; }
archive="${archives[0]}"
appimage="${appimages[0]}"
[ -f "$DIST_DIR/SHA256SUMS" ] || { printf 'BLOCKED: missing Linux checksum manifest in %s\n' "$DIST_DIR"; exit 2; }
tar -tzf "$archive" >/dev/null || { printf 'FAIL: archive is corrupt: %s\n' "$archive"; exit 1; }
root="$(tar -tzf "$archive" | awk -F/ 'NF == 2 && $2 == "pyxross" { print $1; exit }')"
[ -n "$root" ] || { printf 'FAIL: archive has no binary\n'; exit 1; }
for path in "$root/pyxross" "$root/themes/builtin/theme.json" "$root/themes/builtin/atlas.png" "$root/README.md" "$root/LICENSE"; do
  tar -tzf "$archive" | grep -Fx "$path" >/dev/null || { printf 'FAIL: missing %s\n' "$path"; exit 1; }
done
sha256sum -c "$DIST_DIR/SHA256SUMS" >/dev/null || { printf 'FAIL: checksum mismatch\n'; exit 1; }
file "$appimage" | grep -E 'ELF .*executable' >/dev/null || { printf 'FAIL: AppImage is not an ELF executable\n'; exit 1; }
extract_dir="$(mktemp -d)"
trap 'rm -rf "$extract_dir"' EXIT
(cd "$extract_dir" && "$appimage" --appimage-extract >/dev/null) || { printf 'BLOCKED: AppImage extraction/runtime inspection unavailable\n'; exit 2; }
for path in squashfs-root/AppRun squashfs-root/pyxross.desktop squashfs-root/usr/bin/pyxross squashfs-root/usr/share/pyxross/themes/builtin/theme.json squashfs-root/usr/share/pyxross/themes/builtin/atlas.png squashfs-root/assets/pyxross.png; do
  [ -f "$extract_dir/$path" ] || { printf 'FAIL: AppImage missing %s\n' "$path"; exit 1; }
done
printf 'PASS: Linux tar contents, corruption check, and checksum\n'
printf 'PASS: AppImage ELF, extraction, contents, and checksum\n'
printf 'BLOCKED: native GUI launch smoke requires a Linux compositor; Windows smoke requires native Windows/MSVC\n'
