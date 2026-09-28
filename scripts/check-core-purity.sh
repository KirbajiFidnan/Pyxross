#!/usr/bin/env bash
# Core-layer purity gate (CONTEXT.md hard rule 2 / CLI-less CI).
# Fails if any file under src/core imports egui, wgpu, winit, or UI crates.
set -u

CORE_DIR="$(dirname "$0")/../src/core"
forbidden='\b(egui|wgpu|winit|eframe|smithay|wayland)\b'
bad=$(grep -rEn "use\s+(egui|wgpu|winit|eframe|smithay|wayland)(::|\s*;)" "$CORE_DIR" 2>/dev/null)

if [ -n "$bad" ]; then
    echo "PURITY VIOLATION in src/core:" >&2
    echo "$bad" >&2
    exit 1
fi
echo "core purity: OK"