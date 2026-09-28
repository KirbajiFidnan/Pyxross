#!/usr/bin/env bash
# P0 spike runner — examples/projection_spike (SCTK + wgpu layer-shell PoC).
# Usage: scripts/projection_spike.sh [output-name] | --host [output-name] (e.g. DP-1)
# Env: WAYLAND_DISPLAY (default wayland-0), XDG_RUNTIME_DIR (required, Docker mode), P0_IMAGE (default rust:1-bookworm)
set -euo pipefail

MODE=docker
[ "${1:-}" = "--host" ] && { MODE=host; shift; }
ARGS="$*"

# Repo root derived from the script location — safe to call from any cwd.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

if [ "$MODE" = host ]; then
    command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
    exec cargo run --manifest-path "$REPO_ROOT/Cargo.toml" \
        --example projection_spike --release $ARGS
fi

# --- Docker mode ---
if ! command -v docker >/dev/null 2>&1; then
    echo "error: docker not found on PATH — install Docker or run: scripts/projection_spike.sh --host $ARGS" >&2
    exit 1
fi

WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
[ -n "${XDG_RUNTIME_DIR:-}" ] || { echo "error: XDG_RUNTIME_DIR unset — run from a Wayland session" >&2; exit 1; }
SOCKET="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"
[ -S "$SOCKET" ] || { echo "error: Wayland socket not found at $SOCKET" >&2; exit 1; }

IMAGE="${P0_IMAGE:-rust:1-bookworm}"
echo "log: image=$IMAGE; socket=$SOCKET (ro); crate=$REPO_ROOT -> /app/pyx"

MOUNTS=(-v "$XDG_RUNTIME_DIR:$XDG_RUNTIME_DIR:ro")
ENV_ARGS=(-e WAYLAND_DISPLAY="$WAYLAND_DISPLAY" -e XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" -e CARGO_TARGET_DIR=/tmp/target)

if [ -d /dev/dri ]; then
    GPU_ARGS=(--device /dev/dri)
    echo "log: /dev/dri present — hardware Vulkan"
else
    GPU_ARGS=()
    ENV_ARGS+=(-e LIBGL_ALWAYS_SOFTWARE=1)
    LVP_ICD="$(find /usr/share/vulkan/icd.d -name '*lvp*' 2>/dev/null | head -1 || true)"
    if [ -n "$LVP_ICD" ]; then
        ENV_ARGS+=(-e "VK_ICD_FILENAMES=$LVP_ICD")
        echo "log: no /dev/dri — lavapipe via $LVP_ICD"
    else
        ENV_ARGS+=(-e WGPU_BACKEND=gl)
        echo "log: no /dev/dri, no lavapipe — GL backend"
    fi
fi

# Host cargo cache mounted rw: online resolution must be able to write index
# cache + .crate downloads into CARGO_HOME. rw also heals a stale host index
# (first run caches the lockfile versions, later runs resolve offline-speed).
[ -d "$HOME/.cargo/registry" ] && MOUNTS+=(-v "$HOME/.cargo/registry:/usr/local/cargo/registry")
[ -d "$HOME/.cargo/git" ] && MOUNTS+=(-v "$HOME/.cargo/git:/usr/local/cargo/git")

docker rm -f pyxross-spike >/dev/null 2>&1 || true
docker run --rm --name pyxross-spike \
    "${ENV_ARGS[@]}" "${MOUNTS[@]}" "${GPU_ARGS[@]}" \
    -v pyxross-spike-target:/tmp/target \
    -v "$REPO_ROOT:/app/pyx" -w /app/pyx \
    "$IMAGE" \
    bash -c '
        set -e
        apt-get update -qq && apt-get install -y -qq --no-install-recommends \
            libwayland-dev libwayland-client0 libvulkan1 mesa-vulkan-drivers \
            libegl1 libgl1 >/dev/null
        exec cargo run --example projection_spike --release "$@"
    ' _ $ARGS

echo
echo "verify: wayland-info | grep -iE \"layer_shell\"  (expect wlr_layer_shell_v1)"