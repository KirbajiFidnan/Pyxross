# 7. Files, Import & Export

> **In this chapter:** Native `.pyxross` projects, autosave & crash recovery, the built-in File Manager, and every Import & Export workflow (Aseprite, Pyxel Edit, PNG, GIF, Sprite Sheets, Image-to-Pixel).

---

## Native `.pyxross` Project Format

Pyxross saves complete projects (all layers, groups, opacity, blend modes, tile palette, tilemaps, multiple animations, frame delays, palettes, and region size) in two compatible forms:

1. **Single-File `.pyxross` Archive (Default):** A clean, portable ZIP-based file (`my_hero.pyxross`) containing `manifest.json` + lossless PNG layer/tile data.
   - Saved atomically (via a temporary file and backup-protected swap on Windows) so a crash never corrupts your saved file.
2. **Directory `.pyxross` Project:** An unpacked folder containing `manifest.json`, `layers/`, and `tiles/`. Pyxross opens both forms transparently!

### Autosave & Crash Recovery
- Modified documents are periodically autosaved to a recovery journal under your user config folder (`~/.config/pyxross/` on Linux, `%APPDATA%\pyxross\` on Windows).
- If Pyxross closes unexpectedly, the **Recovery** panel prompts you on next startup to restore your unsaved work or discard the recovery snapshot.
- Closing a dirty tab or exiting the app also shows the **Unsaved Changes Guard** (Save / Discard / Cancel, plus Save All when multiple projects are open).

---

## Built-in Cross-Platform File Manager

Pyxross uses its own fast, in-app **File Manager** panel instead of external OS file portals (which avoids desktop-portal hangs on Linux Wayland/tiling WMs):

- **Quick Places Sidebar:** Automatically lists Home, Desktop, Documents, Pictures, Downloads, **Windows drive letters (`C:`, `D:`, …)**, Linux **Root (`/`)**, plus your native **GTK (GNOME/XFCE/Cinnamon)** and **KDE Dolphin** bookmarks!
- **Navigation:** Back / Forward / Up, editable path bar, ancestor dropdown, new-folder creation, and instant image/project previews.
- **Drag & Drop:** You can also drag and drop a `.pyxross`, `.ase`/`.aseprite`, `.pyxel`, `.png`, or `.gif` file straight onto the Pyxross window to open or import it!

---

## Importing Files (`File → Import`)

| Format | Shortcut | What Gets Imported & Options |
|---|---|---|
| **PNG / Image** | `Ctrl+Alt+E` | Opens an image as a new Pyxross document with configurable region/grid slicing |
| **Animated GIF** | `Ctrl+Alt+A` | Decodes GIF frames into a sprite sheet layout and automatically builds a timed animation sequence |
| **Pyxel Edit (`.pyxel`)** | `Ctrl+Alt+P` | Imports layers, nested groups, tile palette, tilemap placements (including flipped/rotated tiles), animations, and palette |
| **Aseprite (`.ase` / `.aseprite`)** | `Ctrl+Alt+G` | Imports layers, groups, lock/visibility flags, palette, and frames — with **Split by Tags** or single-canvas options |
| **Image 2 Pixel** | `Ctrl+Alt+X` | Converts any photo/artwork (PNG, JPG, BMP, WebP, TGA, GIF) into crisp pixel art with live preview, resolution scaling, sampling mode, K-means palette quantization, and dithering |

---

## Exporting Game Assets (`File → Export`)

| Export Type | Shortcut | Best For |
|---|---|---|
| **Export Image (PNG)** | `Ctrl+Shift+E` | Exporting the full composited canvas as a lossless PNG (tilesets, static sheets, icons) |
| **Export Animation** | `Ctrl+Shift+A` | Exporting animations as an **Animated GIF** or **Sprite Sheet PNG + JSON metadata** (frame rects, effective delays, loop settings) |
| **Export Selected** | `Ctrl+Shift+R` | Exporting only the currently selected region or marquee area as a cropped PNG |
| **Export Pyxel (`.pyxel`)** | `Ctrl+Shift+P` | Round-tripping tilesets/layers back to Pyxel Edit |
| **Export Aseprite (`.ase` / `.aseprite`)** | `Ctrl+Shift+G` | Round-tripping layers, groups, and animations to Aseprite or game-engine Aseprite importers (e.g., Godot/Unity plugins) |

---

**Previous:** [← 6. Colors & Palettes](06-color-and-palette.md) | **Next:** [8. Style & Interface Customization →](08-style-and-customization.md)
