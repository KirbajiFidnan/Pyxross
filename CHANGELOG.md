# Changelog

All notable changes to **Pyxross Workshop** are documented in this file.

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
