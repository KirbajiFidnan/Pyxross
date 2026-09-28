# SPIKE P0 Report — Layer-Shell Projection PoC

- **Date:** 2026-09-11
- **Spike revision:** `examples/projection_spike.rs` (current working tree)
- **Compile-verified env:** this container — wgpu 30.0.1 (raw-handle surface creation), smithay-client-toolkit 0.20.0, wayland-client 0.31.15 (`system` feature), calloop, Debian 12 bookworm, Rust stable
- **⚠️ RUNTIME VALIDATION PENDING:** this container has no Wayland socket and no compositor. All runtime criteria below must be validated on the **host Hyprland session** (see [How to run](#how-to-run)). Compile-level evidence is marked ✅; runtime evidence is ⬜ until the host run.

## Success Criteria (P01 §5 Phase 0 + §4.6 constraints)

| # | Criterion (verbatim from plan) | Status | Verification procedure | Notes / spike evidence |
|---|---|---|---|---|
| 1 | `wayland-info` exposes `wlr_layer_shell_v1` (and/or `ext_layer_shell_v1`) on the target session | ⬜ runtime-pending | `wayland-info \| grep -iE "layer_shell"` on the host | Precondition for the whole spike; Hyprland exposes `wlr_layer_shell_v1`. |
| 2 | Surface renders at the requested output + anchor + margins; appears on monitor 2 when requested | ⬜ runtime-pending | `scripts/projection_spike.sh DP-1` (or `--host DP-1`); watch stderr for `spike: targeting output 'DP-1'`; visually confirm top-right, 24px margins, 480×320 | Anchor `TOP\|RIGHT`, margin 24/24/0/0, `exclusive_zone: -1` (no layout shift). Output arg = CLI arg 1. |
| 3 | Alpha translucency correct (grimblast/hyprshot) | ⬜ runtime-pending | `grimblast copy output` (or `hyprshot -m output`) and inspect the corner pixels — checkerboard uses alpha 0.80/0.85, bar is opaque | Premultiplied alpha: shader does `color.rgb *= color.a`; surface uses `CompositeAlphaMode::PreMultiplied` when offered, else `Opaque`. |
| 4 | Frame-callback pacing 55–60 fps sustained; projection keeps rendering while the winit main loop idles | ⬜ runtime-pending | `P0_FRAMES=600 scripts/projection_spike.sh --host`; watch `[P0] fps=...` stderr lines (logged every 2s) — expect 55–60 | Pacing: `wl_surface.frame` callback → render → present → request next frame (§4.6.5). No timers. `MIN_FRAME_INTERVAL` (1ms) only guards double-render within one dispatch cycle. |
| 5 | Empty input region works: clicks pass through (compositor cursor may still render — no cursor-hiding claim) | ⬜ runtime-pending | Click through the surface onto a window beneath; confirm the click lands there | `keyboard_interactivity: None`; explicit empty input region via `wl_compositor.create_region` + `wl_surface.set_input_region` (zero-size region, kept alive for the surface lifetime) — Wayland's default is the full surface, so this call is required. Logs `[P0] input region: empty (click-through)`. |
| 6 | Wgpu surface creation/configure only after first acked layer-shell configure; resize via configure path works | ✅ compile-verified | (runtime) resize the output / change scale and confirm re-present; (compile) `init_gpu` is called only from `LayerShellHandler::configure` | §4.6.2: `gpu` is `None` until the first acked configure; first present happens in the same handler. Later configures re-`surface.configure` with new physical size. |
| 7 | OBS (if available on host): **output capture** of the projection monitor yields clean canvas video — soft criterion, non-blocking | ⬜ runtime-pending | OBS → output capture of the projection monitor; confirm canvas-only, cursor-free video | Not blocking; portal-based output capture excludes the cursor. |
| 8 | §4.6.3 Drop order: wgpu surfaces → SCTK surfaces → `EventQueue` → `Display` | ✅ compile-verified | (runtime) Ctrl-C / close surface; confirm clean exit, no crash; (compile) `main` drops `gpu` then `layer` before the event loop/connection drop | `drop(app.gpu.take()); drop(app.layer.take());` before loop teardown; `create_surface_unsafe` lifetime contract documented at the call site. |
| 9 | §4.6.5 Frame pacing: render → present → request frame → wait → (skip if version unchanged). No timers | ✅ compile-verified | (runtime) see criterion 4; (compile) `CompositorHandler::frame` → `render()` → `frame()` request → wait | Frame callback requested **before** `present` (wgpu Wayland contract — same commit cycle). |
| 10 | §4.6.6 Surface lifecycle: compositor destroys layer-shell surface → recreate or degrade, **never panic**; fd EOF = clean exit | ✅ compile-verified | (runtime) `hyprctl dispatch closewindow` on the spike surface, or kill the compositor; (compile) `LayerShellHandler::closed` sets `exit = true`; `surface lost` path exits | `closed` → clean exit (no panic). `CurrentSurfaceTexture::Lost/Validation` → `spike: surface lost, exiting`. |

## How to Run

### Docker (default — matches CONTEXT.md §7 dev env)

```bash
scripts/projection_spike.sh [output-name]        # e.g. scripts/projection_spike.sh DP-1
```

- Requires: Docker, a Wayland session (`XDG_RUNTIME_DIR` set, `WAYLAND_DISPLAY` defaults to `wayland-0`), and the host cargo cache (`~/.cargo/registry`, `~/.cargo/git`) for the offline build.
- Mounts: host `$XDG_RUNTIME_DIR` (ro), crate root `$(pwd)` → `/app/pyx` (workdir), `/dev/dri` when present (else lavapipe via `VK_ICD_FILENAMES`, or `WGPU_BACKEND=gl` fallback).
- Image: `rust:1-bookworm` (override with `P0_IMAGE`). Container installs `libwayland-dev libwayland-client0 libvulkan1 mesa-vulkan-drivers libegl1 libgl1`, then `cargo run --offline --example projection_spike --release`.
- Run from the repo root (mount derives from `$(pwd)`).

### Direct on host

```bash
scripts/projection_spike.sh --host [output-name]
```

Builds and runs with `cargo run --offline --example projection_spike --release` (release avoids debug slowness for fps checks; debug works too).

### Env / CLI reference (what the example actually accepts)

| Knob | Form | Default | Effect |
|---|---|---|---|
| Output name | CLI arg 1 (`DP-1`) | primary output | Target output for the layer surface; logs `spike: targeting output 'NAME'` |
| `P0_SIZE` | env `WxH` | `480x320` | Logical size (physical = logical × buffer scale) |
| `P0_FRAMES` | env `N` | `0` (unlimited) | Auto-exit after N rendered frames |
| Anchor / margins / layer | constants | `TOP\|RIGHT`, 24/24/0/0, `Overlay` | Edit the constants in the example to change |

**stderr milestones:** `spike: running (layer-shell projection)` · `spike: targeting output 'NAME'` · `[P0] input region: empty (click-through)` · `[P0] fps=N.N` (every 2s) · `spike: surface lost, exiting` · `spike: layer surface closed by compositor` · `spike: exited after N frames`.

## Feed to D65/D66

**Proven now (compile-level):**
- wgpu 30 raw-handle surface creation from SCTK handles (`create_surface_unsafe` + `RawDisplayHandle::Wayland`/`RawWindowHandle::Wayland`) compiles and links against wgpu 30.0.1 + wayland-client `system` feature.
- SCTK 0.20 layer-shell API surface (`LayerShell::create_layer_surface`, `Anchor`, `KeyboardInteractivity`, `LayerSurfaceConfigure`) is usable; `delegate_*` macro wiring compiles.
- Drop-order structure (§4.6.3) and first-present-after-acked-configure (§4.6.2) are enforced by construction (gpu init only in `configure`; explicit drop order in `main`).
- Frame-callback pacing loop (§4.6.5) compiles with the wgpu Wayland present-ordering contract.

**Stays gated on the host run (Hyprland):**
- Actual rendering + alpha translucency (criterion 3), 55–60 fps pacing (criterion 4), click-through (criterion 5), output targeting (criterion 2), OBS clean capture (criterion 7).
- If the host run fails, D65's layer-shell-first decision is superseded per DECISIONS.md (spike failure supersedes the decision) — the degradation ladder (`wlr-layer-shell` → `ext-layer-shell` → `xdg-toplevel` → winit frameless) is the fallback.