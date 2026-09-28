# Roadmap

> **Status:** Technical reset (2026-09-10, D64). Previous Godot implementation (M0–M6) **abandoned** — milestones renumbered R0… for the Rust rewrite. Nothing implemented yet.
> Timeline assumes focused work; adjust as needed (user-driven, no CI/git).

## Milestones

| Phase | Scope | Target | Status |
|---|---|---|---|
| **P0** | Projection spike (throwaway PoC): SCTK + wgpu layer-shell surface on Hyprland (via Docker host-socket mount) rendering a test pattern on a chosen output — validates D65/D66 implementation viability. Gate: P01 §5 criteria | wk 0 | ✔ artifacts (examples/projection_spike.rs, scripts/projection_spike.sh, docs/SPIKE_P0_REPORT.md); runtime gate ⬜ (host Hyprland run pending) |
| **R0** | Cargo workspace skeleton: crate layout, `cargo test` harness, core types (PixelBuffer, Rect2i, Color), manual winit + egui + wgpu "blank window" bootstrap [D66]; `Projector` core interface + `WinitProjector` + PreviewManager skeleton (View → New Preview Surface) | wk 1 | ✔ |
| **R1** | Core render base: PixelBuffer, ChunkManager (256×256 pages), straight-alpha Compositing, Camera (integer zoom 10–3200%), CanvasRenderer (wgpu chunk upload, dirty rects), pencil/eraser + brush model + undo (reverse-delta commands); **ProjectionRenderPass** (shared canvas texture → N surfaces, version-counter pacing) + Capture Mode (Window-primary per D67: OBS window capture; Overlay opt-in) | wk 2–4 | ✔ render base + UI wiring (224 tests); Capture Mode ⬜ (P0-gated) |
| **R2** | Tools: fill (contiguous + replace-all), eyedropper (composite read, right-click temp), select + move, clipboard (internal + PNG); layer stack core: Layer/LayerStack/LayerStackController + structural undo, Layer-aware compositing (visibility/opacity/blend) | wk 5–8 | ✔ tools + layer stack (392 tests, core purity OK); fill replace-all ⬜ (UI wiring pending) |
| **R3** | Regions + frame model, timeline + playback panel, onion skin, marching-ants; **PlaybackView + FrameEditView projections, monitor picker** [D65] | wk 8–11 | ✔ frame model + timeline + onion/ants + ViewSource/monitor picker (480 tests, core purity OK); projection surface creation + Capture Mode wiring ⬜ (P0-gated) |
| **R4** | Transform object: RotSprite + exact transforms (R/H/V keys) + mini-stack + hybrid preview + gizmo + session state machine (Ctrl+LMB enter, commit/cancel) | wk 10–13 | ✔ tamamlandı (557 tests, 0 fail, clippy 3 pre-existing unrelated warnings, purity OK). U16 ✔ RotSprite + exact; U17 ✔ TransformObject mini-stack; U18 ✔ gizmo geometry/paint/hit-test (BBox/GizmoHit) + CanvasOverlay::Gizmo + interaktif transform session (lift/drag/commit/cancel) + gesture'lar (R/CW, Shift+R/CCW, H flip, V flip, +/- scale) + Esc-cancel + apply_layer_commits + flip-commit session tests. |
| **R5** | 9-slice skin system complete + default theme + user theme loading + theme browser; fixed panel layout polish (toolbox/layers/timeline/color-palette panels) | wk 13–15 | ✔ theme system (589 tests, 0 fail, clippy pre-existing warnings only, purity OK). F1 ✔ Theme core types (ThemeColors 16 tokens, NineSlice, Skin, Theme, ThemeManager, load_atlas; src/ui/theme.rs) + default theme assets (assets/themes/default/theme.json + atlas.png); F5 ✔ hardcoded chrome colors → theme reads (GizmoColors in render/gizmo.rs, ClearColor from theme, invariant test no_hardcoded_chrome_colors_in_ui_or_render); F3 ✔ user theme loading (PYXROSS_THEMES → themes/user → themes/builtin, first-dir-wins, malformed skip, builtin Dark fallback) + gap tests; F4 ✔ theme browser (src/ui/settings.rs: swatch preview strip, theme combo, ⟳ refresh, error line, live switching); F6 ✔ panel layout polish (panel_frame() 4pt uniform inner margin, SECTION_SPACING/ITEM_SPACING named constants). 9-slice skinned rendering (skins→atlas paint) ⬜ deferred (tokens/manager ready; skin painting not wired to widgets yet). |
| **R6** | Persistence: .pyxross directory format, autosave (5 min), recovery journal, New/Open/Save dialogs | wk 15–17 | F1 ✔ .pyxross format core+io (Document/LayerDoc/RegionDoc/FrameDoc/SequenceDoc/EditorSettings, save_document PNG-first+manifest, load_document + version migrate; src/core/document.rs + src/io.rs, 604 tests); F2 ✔ New/Open/Save/Save As dialogs (rfd folder pickers, File menu), dirty-flag tracking (mark_dirty wired into stroke/fill/undo/redo/cut/paste/select-move/transform/layer/timeline mutations), window-title dirty marker, New-canvas modal (w/h/tile DragValues), unsaved-changes guard (Save/Discard/Cancel via egui Modal + deferred PendingAction::{New,Open,Close}), CloseRequested guard, app_to_document ↔ document_to_app round-trip + tile_size persistence + texture re-upload on load (612 tests, clippy clean); F3 ✔ autosave (5-min interval to config dir `pyxross/autosave/<project>/`, copy-only, never touches current_path/modified, io.rs autosave_dir_for/sanitize_project_name + ui should_autosave/perform_autosave); F4 ✔ recovery journal (RecoveryJournal serde struct, write/read/clear_recovery_journal in io.rs, restore/discard modal at launch via recovered-unsaved-work dialog, journal cleared on manual save/new/load, 625 tests, clippy pre-existing only); F5/F6 ⬜ |
| **R7** | Export suite: sprite sheet + JSON meta + GIF + **Export Selected**; PNG import | wk 17–18 | ✔ implementation complete; local verification pending (Cargo unavailable) |
| **R8** | Palette system, keybinding editor (JSON defaults + user overrides), polish pass | wk 18–20 | 🟨 |
| **R9** | Big-canvas performance: checked large-canvas arithmetic, all-layer dirty-region sync, RotSprite preview threshold, grid anti-moiré, profiling (criterion) | wk 20–22 | 🟨 implementation complete; Rust verification blocked by unavailable cargo/rustc |
| **R10** | Packaging: portable Windows `.exe`, Linux `.AppImage` + `.tar.gz`; release notes | wk 22–24 | ✔ implementation complete; native toolchain/AppImage/GUI verification blocked (see [RELEASE_TESTING.md](RELEASE_TESTING.md)) |

## Post-MVP (ordered)

1. PNG import polish → 2. Aseprite import → 3. Pyxel Edit import
4. Dock/float-panel system (D60–D63 design retained)
5. Linked tiles (live instance updates)
6. Binary single-file export format (decision deferred)
7. Video export (APNG/WebP/MP4), macOS packaging, indexed color — all deferred, architecture must not preclude

## Rule

First implementation package: **P0 spike** (user go-ahead) then **R0 skeleton** (user-approved through the proposal–approval loop). R1+ planned separately through the proposal–approval loop.

## Milestone Kickoff

Every milestone starts with the kickoff workflow defined in [CONTEXT.md §6.1](../CONTEXT.md#61-milestone-kickoff-workflow-binding-for-every-milestone): work summary → per-feature approval via selectable options → "Self-debate" option available on every feature question (architecture-first reasoning) → implementation only after approvals.
