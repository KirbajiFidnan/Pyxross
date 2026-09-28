# Architecture

> **Status:** Technical reset (2026-09-10). Previous Godot implementation abandoned (D64). All sections below are **design targets** — nothing implemented yet in the Rust stack (R0 planned via approval loop).

## 1. Project Layout

```
/app/pyx/
├── Cargo.toml
├── CONTEXT.md / docs/
├── assets/            # themes (default atlas+JSON), palettes, keybinding defaults
├── src/
│   ├── core/          # pure logic, NO UI dependencies (egui/wgpu/winit)
│   │   ├── buffer/    # PixelBuffer, ChunkManager
│   │   ├── model/     # Layer, LayerStack, Region, Frame, AnimationSequence
│   │   ├── anim/      # AnimationController, OnionConfig (playback core)
│   │   ├── transform/ # RotSprite, flips, NN scale, compositing
│   │   ├── projection.rs # Projector interface, opaque ids, event channel (wgpu-free) [D65]
│   │   └── undo/      # Command base, UndoStack, composite commands
│   ├── render/        # wgpu: upload, chunk draw, overlays (ants, gizmo, onion)
│   │   ├── canvas.rs  # canvas texture, chunk management
│   │   ├── overlay.rs # selection ants, transform gizmo, onion ghosts
│   │   ├── projection.rs # projection quad pass: shared canvas texture → N surfaces [D65]
│   │   └── shaders/   # WGSL shaders
│   ├── platform/      # platform/OS backends (no core deps; egui/winit/wgpu allowed)
│   │   ├── wayland/layer_shell.rs  # SCTK layer-shell backend [D65]
│   │   └── winit_projection.rs     # winit frameless fallback (GNOME/Windows/X11) [D65]
│   ├── ui/            # egui: panels, canvas widget, skin system, projections manager
│   │   ├── canvas.rs  # canvas widget (wgpu texture → egui)
│   │   ├── toolbar.rs # tool selection + color/palette
│   │   ├── layers.rs  # layer panel
│   │   ├── timeline.rs# animation timeline + playback controls
│   │   ├── projection.rs # PreviewManager (projection list, view sources, monitor picker) [D65]
│   │   └── theme.rs   # 9-slice skin system
│   ├── input.rs       # tool logic, gestures, keybindings
│   ├── io.rs          # .pyxross format, PNG import/export
│   └── main.rs        # app entry, event loop, app state
└── tests/
    ├── unit/          # cargo test, core-layer only
    └── manual/        # manual verification scripts (--manual flag)
```

**Folder rules:**
- `src/core` never imports egui/wgpu/winit or any UI crate (CONTEXT hard rule 2).
- `src/platform` may import wgpu/winit/egui but never imports `src/ui` (backends implement core interfaces only).
- Hybrid organization: layer on top, feature subfolders inside layers.

## 2. Wiring & Ownership

- **App state** (`main.rs`): owns `Canvas`, `LayerStack`, `UndoStack`, `ToolState`, `AnimationController`, `ThemeManager`, `Camera`, `PreviewManager`.
- **PreviewManager** (ui/projection.rs): owns the active projection list; maps each projection to a view source (`EditorCanvasView` / `PlaybackView` / `FrameEditView` — projections of view state, never a second document); exposes monitor picker + Capture Mode toggle [D65].
- **Borrow discipline:** core state is owned by the app; UI gets `&mut` access per frame. No global singletons (Rust ownership makes this natural).
- **Event flow:** `winit` events → `App.handle_event()` → input/tool state machine → core mutations → repaint.
- **Change propagation:** change-signals, never polling (same as [D29]).

## 3. Data Model

**Layered canvas** [D04]:
- Each layer owns a full-canvas pixel buffer (chunked via `ChunkManager`); chunk page and dirty metadata are sparse, so untouched pages do not allocate per-canvas slot metadata.
- Frames and tiles are **rect references** into the canvas: `Region` = `{rect: Rect2i, name}`. Drawing on canvas updates all referencing frames automatically — no per-frame pixel copies.
- **Playback:** `AnimationController` — deterministic `advance(delta_ms)`, `play()`/`stop()`/`restart()`, `current_index`, change notifications via `Changed` events (no scene-tree timer; the app loop drives it).
- **Onion skin:** `OnionConfig` — prev/next frame counts (0..3), tint colors (prev reddish, next greenish); renderer composites neighbouring frame regions with tint overlay.
- **Layer stack:** `Layer` = name + visible + opacity (0–1, clamped) + blend (NORMAL/MULTIPLY/SCREEN/ADD) + `PixelBuffer`. `LayerStack` owns ordered array, active layer tracked **by object identity**, one `changed_event()` per mutation. `LayerStackController` facade: every mutation → exactly one undoable `Command`.
- One document = one canvas + one layer stack.
- Selection/transform scope: active layer only [D36]; multi-layer opt-in per D32.

## 4. Transform Object (see FEATURES.md for full spec)

- Ctrl+LMB lifts the selection into a floating, layer-less transform object rendered above everything. [D32]
- **Mini-stack:** internally holds per-layer buffers (not one flattened buffer).
  - Single-layer: commit writes into the layer active at commit time (free cross-layer move).
  - Multi-layer (opt-in): each buffer returns to its source layer; no merging, no data loss; undo = one composite command.
- State machine: `idle → transforming → committed/cancelled`. Details in FEATURES.md §3.

## 5. Rendering Pipeline

- Core `Compositing` (pure, headless-testable) composites layers bottom-to-top with straight-alpha "over" into a fresh RGBA8 `Vec<u8>` — `composite()` (full canvas) and `composite_region()` (arbitrary canvas-space rect).
- `CanvasRenderer` (render layer, wgpu) splits the canvas into 256×256 chunks [D40]; each chunk is composited into a wgpu `Texture` via `queue.write_texture`.
- Dirty rectangles from every layer's `PixelBuffer` are unioned at sync time, then the affected region is re-composited across the complete layer stack. This preserves non-active-layer edits while avoiding work for untouched frames; the existing 256×256 chunk boundary remains the paging unit.
- `Camera` owns zoom/pan state + integer-snap math (10%–3200%, nearest-neighbor everywhere) [D28]; renderer draws only viewport-intersecting chunks.
- **Overlays:** selection marching-ants border (animated dashed magenta rect), onion skin ghosts (prev reddish/next greenish), transform gizmo (bbox + 4 corner handles + pivot crosshair).
- Tile grid overlay: always visible by default, toggleable [D41].
- **Projection surfaces [D65]:** one wgpu `Instance`/`Device` serves the main window + N projection surfaces. Projections render the **shared canvas texture** via a fullscreen textured quad (nearest-neighbor, integer scale `floor(monitor_size / canvas_size)`). Canvas texture usage: `COPY_DST | TEXTURE_BINDING`, updated `queue.write_texture` on change-signal (D29); shared as `Arc<wgpu::Texture>` + `AtomicU64` version counter (a projection re-presents only when the canvas version changed). Build against the `egui_wgpu::wgpu` re-export — exactly one pinned wgpu version. Details: §9.

## 6. UI Panels (fixed layout, v1) [D64]

```
┌─────────┬──────────────────────┬───────────┐
│ Toolbox │                      │ Layers    │
│ (tools) │      Canvas          │ (list)    │
│ Color/  │   (center, clipped)  │           │
│ Palette │                      │           │
├─────────┴──────────────────────┴───────────┤
│ Timeline · Playback                         │
└─────────────────────────────────────────────┘
```

- Left: toolbox + color/palette. Right: layers + frame list. Bottom: timeline + playback. Center: canvas.
- The canvas widget is embedded in a clipped center region and never overpaints panels.
- Panel separators are draggable (sizes adjustable); dock/float system deferred post-MVP [D64].

## 7. Change Notification Map

| Notification | Emitted by | Consumers | Status |
|---|---|---|---|
| `pixels_changed(rect)` | `PixelBuffer` buffer ops | CanvasRenderer (dirty chunks) | ⬜ R1 |
| `frame_changed(index)` | `AnimationController` | CanvasRenderer (onion rebuild), TimelinePanel (highlight) | ⬜ R3 |
| `playback_started`/`stopped` | `AnimationController` | TimelinePanel transport state | ⬜ R3 |
| `onion_changed` | `OnionConfig` | CanvasRenderer (onion rebuild) | ⬜ R3 |
| `tool_changed(tool)` | Input layer (deliberate switches only) | Toolbar highlight, canvas cursor, status hints | ⬜ R1 |
| `layer_stack_changed` | `LayerStack` (any mutation) | renderer (recomposite), layer panel / preview (relist) | ⬜ R2 |
| `regions_changed` | region manager | frame list, tile panel | ⬜ R3 |
| `theme_changed(spec)` | ThemeManager | all UI (single reskin owner) | ⬜ R4 |
| `surface_created`/`surface_destroyed`/`surface_resized` | Projector backend | PreviewManager / UI (projection list, monitor picker) | ⬜ R1 |
| `surface_died` (compositor destroyed the surface) | Projector backend | PreviewManager (recreate or degrade — never panic) | ⬜ R1 |

## 9. Projection System (Detached Canvas Surfaces) [D65, D66]

### 9.1 Interface & backends

- **Interface in `src/core/projection.rs`** — wgpu-free, headless-testable: `Projector` trait with `create(target: MonitorTarget, mode: ProjectionMode) -> ProjectionId`, `set_monitor`, `set_capture_mode`, `destroy`, and an event channel (`surface_created/destroyed/resized/died`). Opaque ids only — purity rule holds.
- **Backends in `src/platform/`:**
  - `LayerShellProjector` — SCTK, `wlr-layer-shell` (ext opportunistic). Linux: Hyprland, wlroots family, KWin. Capabilities: programmatic monitor targeting (`set_output`), overlay layer (always-on-top), alpha, click-through.
  - `WinitProjector` — winit frameless viewport. GNOME fallback (no layer-shell; manual drag, no always-on-top — core protocol limitation), Windows (exact placement + topmost + normalize), X11.
- **Degradation ladder** (chosen at startup, logged): `wlr-layer-shell` → `ext-layer-shell` (staging, unconfirmed on Mutter/Hyprland — opportunistic only) → `xdg-toplevel` (WinitProjector) → Windows frameless. The feature never disappears (R4); capability degrades.
- **Capability matrix (honest):**

| Capability | LayerShell (Hyprland/wlroots/KWin) | Winit fallback (GNOME) | Winit (Windows) |
|---|---|---|---|
| App-controlled monitor targeting | ✅ | ❌ (manual drag) | ✅ |
| Always-on-top | ✅ (overlay layer) | ❌ | ✅ |
| OBS capture | ⚠️ output capture (window capture lists xdg-toplevels only) | ✅ window capture | ✅ window capture |
| Click-through (capture mode) | ✅ (empty input region) | ✅ | deferred (needs WS_EX_*) |

### 9.2 Event loop (D66)

- Main window: winit `EventLoop` + egui-winit + egui-wgpu (manual integration, no eframe).
- **Dedicated wayland thread** owns `Display` + `EventQueue` + all SCTK state; calloop loop with two sources: the `EventQueue` (`calloop::Source`) + a command `Channel` (winit→wayland: create/destroy/set-monitor/capture-mode).
- **Rendering on the wayland thread**, compositor-paced: render → present → request `wl_surface.frame` → on callback re-render only if the canvas version counter changed. Main loop may idle indefinitely; projections keep the projection monitor's refresh.
- wayland→winit: `EventLoopProxy::send_event` for lifecycle only. Exit: shutdown via channel → drop (order below) → `join`. Wayland fd EOF (compositor gone) = clean exit.
- `wgpu::Instance` created before the wayland thread; `Arc<wgpu::Device>` + `Arc<wgpu::Texture>` shared (Send+Sync; `Queue::submit` thread-safe).

### 9.3 Capture Mode (OBS-clean)

- Canvas only: no HUD, grid/guides/onion forced off, exact pixel alignment.
- Empty input region (`wl_surface.set_input_region(∅)`) + `keyboard_interactivity: none` = **click-through** (not cursor-hiding — the compositor owns the cursor).
- **OBS on Wayland = output capture:** portal window capture enumerates xdg-toplevels only; use a full-monitor layer-shell projection + OBS "Screen Capture (PipeWire)" of that monitor. Portal capture excludes the cursor → clean video. **OBS on Windows = window capture** of the frameless `WinitProjector`.

### 9.4 Binding technical constraints

1. Create projection wgpu surfaces **on the wayland thread**; the thread outlives all its surfaces.
2. No surface creation/configure before the first acked layer-shell `configure` (size 0 before); `wl_surface.set_buffer_scale`; physical-pixel sizing; first present after acked configure; configure-with-new-size → `surface.configure` + re-present.
3. Drop order: wgpu surfaces → SCTK surfaces → `EventQueue` → `Display` (wayland thread); device/instance on main thread after `join`.
4. Never two wgpu surfaces on one `wl_surface`; `create_surface_unsafe` safety contract documented on the caller.
5. Frame pacing via `wl_surface.frame` callbacks — no timers.
6. Compositor can destroy surfaces (output removal / session end) → `surface_died` → recreate or degrade, never panic.
7. One `egui_wgpu::Renderer` per surface format — v1 needs one (main window).
8. Renderer and quad pass share the device without conflict (separate pipelines/bind groups).

## 8. Conventions

- Idiomatic Rust: `cargo fmt`, `cargo clippy` clean; snake_case files, PascalCase types.
- Core layer portable: allowed types are `Vec<u8>`, math types, our own types. No wgpu/egui/winit imports.
- No hard-coded UI colors — everything through the theme system (CONTEXT hard rule 3).
- Each feature lands with its unit tests (`cargo test`), core-layer only.
