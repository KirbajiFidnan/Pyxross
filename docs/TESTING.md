# Testing Strategy

> **Status:** Design targets only (Rust rewrite, R0+). Previous GUT/Godot suite abandoned with the stack reset (D64).

## Scope [D22]

- **`cargo test`** — `tests/` + inline `#[cfg(test)]` modules.
- **Core layer only:** deterministic logic tests (no egui/wgpu/winit in tests). UI is verified manually by the user. No CI, no git hooks.

| Test target | What is covered |
|---|---|
| PixelBuffer / ChunkManager / Compositing / Camera | alloc, read/write across chunk edges, dirty-rect correctness, 256×256 pages, alpha-over compositing, integer-zoom ladder |
| Undo / tools / select / clipboard | reverse-delta Command/CompositeCommand/UndoStack, DeltaRecorder, pencil/eraser stroke, fill (contiguous + replace-all), eyedropper, selection+move, clipboard+PNG |
| Model / animation | Region/Frame/AnimationSequence/Layer, AnimationController (playback, loop, change events), OnionConfig (defaults, clamping) |
| Exact transforms | rotate 90/180/270 + H/V flips pixel-exact, identity round-trips |
| RotSprite | 0° identity, 90° equals exact rotation, determinism, palette purity/no interpolation |
| TransformObject | lift clears source, commit same/cross-layer, multi-layer returns to own layers, undo round-trip, cancel verbatim, preview consistency |
| LayerStack / structural undo | Layer add/insert/remove/move/duplicate semantics, active-by-identity, opacity/blend clamping, Layer-aware compositing exact math, structural command undo/redo round-trips |
| .pyxross round-trip | save → load → lossless compare; manifest version migration |
| Theme system | manifest validation, fallback chain, atomic switch, malformed-theme resilience, **no-hard-coded-color invariant** |
| R8 palette/keymap | palette validation and JSON round-trips, deterministic user precedence, keymap merge/conflict validation, configurable shortcut dispatch, grid visibility state |
| Fit zoom | ladder fit + degenerate defaults |
| R9 performance safety | checked buffer/rectangle arithmetic, lazy chunk boundaries, region-vs-full compositing equality, 64px transform preview boundary, and viewport-cullable grid helpers |
| Projection (core interface) | Projector trait contract against a mock backend: create/set_monitor/set_capture_mode/destroy, opaque-id lifecycle, event-channel sequence (created→resized→died), MonitorTarget/ProjectionMode mapping — wgpu-free by construction |

## Conventions

- Test command (from project root): `cargo test`.
- Core tests never require a window, input device, or display.
- Reference-pattern tests: input pixel matrix + expected output matrix inline in the test file (no binary fixtures unless size demands).
- Manual verification scripts live in `tests/manual/` and print PASS/FAIL — not part of the unit suite.
- `cargo fmt --check` and `cargo clippy` clean are required for every merged change.
- Criterion profiling is optional until the Rust toolchain is available; benchmark results must be recorded from an actual run, never estimated.
- R10 packaging commands and the portable archive/AppImage checklist live in [RELEASE_TESTING.md](RELEASE_TESTING.md). Packaging smoke checks require real artifacts and never treat a missing toolchain or binary as a pass.

## Manual Testing (user)

Each milestone ships a short manual checklist (what to click, what to expect). The user verifies visuals/feel on Hyprland; automated suite guarantees core correctness.

### Projection probe checklist ([D65], as surfaces land)

1. **R1 — Preview surface:** View → New Preview Surface opens a frameless canvas-only window; canvas edits appear live; Fit / Follow / Fixed camera modes behave.
2. **R1 — Capture Mode (Linux/Hyprland):** full-monitor projection + OBS "Screen Capture (PipeWire)" on that monitor → recorded video shows only the canvas (no HUD); monitor change via the picker works.
3. **R0/R1 — degradation ladder:** app startup log (or `--print-capabilities`) shows the chosen rung (`wlr-layer-shell` / `ext-layer-shell` / `xdg-toplevel`).
4. **GNOME fallback (if session available):** preview appears as an ordinary draggable frameless window; OBS window capture selects it; no always-on-top (expected).
5. **R3 — Playback/Frame-edit surfaces:** playback syncs to the timeline without jitter; a projection on a monitor with a different refresh rate runs at that monitor's rate; closing a projection never affects document state.
6. **Robustness:** unplug the projection's monitor (or end the compositor session) → app logs `surface_died` and degrades cleanly — no panic.
