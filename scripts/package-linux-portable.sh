#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGE_TAG="${PYXROSS_BUILDER_IMAGE:-pyxross-linux-portable:ubuntu2204}"
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$REPO_ROOT" log -1 --format=%ct 2>/dev/null || date +%s)}"

command -v docker >/dev/null 2>&1 || {
  printf 'error: docker is required to build the portable GLIBC 2.35 Linux release\n' >&2
  exit 1
}

docker build -t "$IMAGE_TAG" -f "$REPO_ROOT/packaging/Dockerfile.linux-portable" "$REPO_ROOT/packaging"

EXTRA_MOUNTS=()
if [ -d "${CARGO_HOME:-$HOME/.cargo}/registry" ]; then
  EXTRA_MOUNTS+=(-v "${CARGO_HOME:-$HOME/.cargo}/registry:/tmp/cargo/registry")
fi

# Use a separate container-side target directory so host/container Cargo artifacts do not collide.
docker run --rm \
  -u "$(id -u):$(id -g)" \
  -e "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH" \
  -e "CARGO_TARGET_DIR=/work/target-linux-portable" \
  -e "CARGO_HOME=/tmp/cargo" \
  -e "HOME=/tmp" \
  -e "ARCH=x86_64" \
  -v "$REPO_ROOT:/work" \
  "${EXTRA_MOUNTS[@]}" \
  -w /work \
  "$IMAGE_TAG" \
  /work/scripts/package-linux.sh

printf 'Verifying maximum GLIBC symbol version...\n'
MAX_GLIBC="$(objdump -T "$REPO_ROOT/target-linux-portable/x86_64-unknown-linux-gnu/release/pyxross" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -Vu | tail -n 1)"
printf 'Max GLIBC symbol: %s\n' "${MAX_GLIBC:-none}"
