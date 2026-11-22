# Changelog

All notable changes to **Pyxross Workshop** are documented in this file.

## [0.9.1] - 2026-11-22

### Added
- **File Manager:** sortable `Name`, `Size`, and `Modified` columns (ascending/descending), live autocomplete search in `File name` (including extension matching like `.png` with `Tab` completion), and a `File type` filter dropdown.
- **Image-to-Pixel:** smooth pointer-centered preview zoom & pan with `Fit` reset, plus `Save Preset` and `Load Preset` (`.json`) controls in the `Size` tab.
- **Import Banner Panel:** new two-column banner importer launched from `Select Banner` with `Banner Name` input, `Cut (8:1)` / `Full Image` modes, zoomable/pannable preview, draggable 8:1 crop frame with shaded exterior, and direct PNG export into `banners/`.
- **Top-Bar Banner Shortcut:** invisible clickable hotspot on the top-bar banner region with a subtle `"Click to change banner"` hover hint in the top-right corner.
- **Style Panel:** ergonomic `Save` (enabled when customized) and `Load` theme dialog workflow and a `Spacing` slider.
- **Key Bindings Panel:** redesigned with live search, category filter tabs (`Tools`, `Edit`, `Transform`, `File`, `Import / Export`, `View`), click-to-record monospace keycap badges, inline conflict indicators, manual key/modifier popover, and a pinned `Reset` / `Apply` footer.

### Fixed
- **Windows Input & Render Latency:** configured swapchain `desired_maximum_frame_latency = 1`, added `window.pre_present_notify()`, and flushed canvas texture uploads in the same frame after input handling to eliminate multi-frame cursor lag.
- **Overlapping Panel Z-Order:** clicking the front floating panel (or foreground popups) no longer raises occluded background panels or triggers background header drags.
- **Fast Canvas Edge Strokes:** rapid inward and outward brush swipes now interpolate cleanly across canvas borders without missing edge pixels while staying paused during off-canvas motion.
- **Panel Layout & Scrollbars:** fixed narrow-panel slider/button overlap in `Style`, pinned top action buttons in resizable `Select Banner`, eliminated resize scrollbar flicker and text overflow in `File Manager`, and kept `Key Bindings` footer buttons clear of scrollbars.
- **Pencil Default Shortcut:** guarded test keymap persistence so unit tests cannot overwrite user `keybindings.json` (keeping Pencil on `D`).

## [0.9.0] - 2026-11-20

### Added
- Dockable and floating panel workspace (`egui_dock`) with top bar, menus, and customizable pixel-art banners.
- Single-file `.pyxross` ZIP archive format (alongside directory projects) with autosave and crash recovery.
- **Aseprite** (`.ase` / `.aseprite`) and **Pyxel Edit** (`.pyxel`) import and export.
- Animation dock suite: multiple animations per project, live Playback panel, Frame Editor with onion skinning, and sprite sheet / GIF export.
- **Image-to-Pixel** converter (sampling, K-means palette extraction, dithering), inspired by `SLK_img2pixel`.
- Built-in cross-platform **File Manager** with OS standard folders, Windows drive enumeration, and Linux GTK2/3 + KDE Dolphin bookmarks.
- Pixel-perfect rotation and transform algorithms (**RotSprite**, **CleanEdge**, **Rotxel**) plus exact 90°/180°/270° and flip operations.
- Cross-platform Linux (`GLIBC 2.35` portable AppImage & `.tar.gz`) and Windows (`x86_64-msvc` portable `.zip`) automated release packaging.

### Improved
- GPU & swapchain resilience: `Outdated`/`Lost` surface reconfiguration, Windows minimize (`0x0`) guard, software adapter fallback (WARP / llvmpipe), native window icon, and Linux Wayland `app_id` / X11 `WM_CLASS`.
- Safe two-phase backup file replacement on Windows when saving `.pyxross` projects.
- Embedded **JetBrains Mono Nerd Font** with full SIL OFL 1.1 attribution.
