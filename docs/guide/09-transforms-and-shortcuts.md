# 9. Transforms, Shortcuts & Troubleshooting

> **In this chapter:** Pixel-perfect rotation and scaling (**RotSprite / CleanEdge / Rotxel**), camera controls, the complete default keyboard shortcut reference, and cross-platform troubleshooting tips.

---

## Pixel-Perfect Transforms & Rotation

When you drag inside a selection (or press **`Ctrl+V`** to paste), Pyxross lifts those pixels into a live **Transform Session** with interactive canvas gizmos (move, corner scale, edge resize, and free rotation handle).

While a transform is active, the **Tool Property** panel lets you pick the pixel-art rotation algorithm:

| Algorithm | Characteristics |
|---|---|
| **RotSprite** | Classic structure-preserving pixel-art rotation (upscale → rotate → clean downsample); crisp and reliable for most sprites |
| **CleanEdge** | Edge-aware pixel rotation that preserves sharp silhouettes and diagonal contours cleanly |
| **Rotxel** | Palette-pure, binary-alpha pixel-art rotation optimized for clean pixel alignment |

### Transform Controls
- **Rotate 90° CW / CCW:** **`R`** / **`Shift+R`** (exact, lossless)
- **Grow / Shrink:** **`+`** / **`-`**
- **Flip Horizontal / Vertical:** **`H`** / **`V`** (exact, lossless)
- **Commit Transform:** Click empty space outside the transform box (or double-click empty space) — commits cleanly onto the currently active layer as one undoable edit!
- **Cancel Transform:** **`Escape`** restores the original pixels byte-for-byte.

---

## Complete Default Keyboard Shortcuts

### File & Project
| Action | Default Shortcut |
|---|---|
| New Document | `Ctrl+N` |
| Open Project | `Ctrl+O` |
| Save Project | `Ctrl+S` |
| Save Project As | `Ctrl+Shift+S` |
| Resize Canvas | `Alt+R` |

### Edit & Selection
| Action | Default Shortcut |
|---|---|
| Undo / Redo | `Ctrl+Z` / `Ctrl+Y` |
| Copy / Cut / Paste | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| Delete Selection | `Delete` |
| Invert Selection | `Ctrl+I` |
| Flip Selection H / V | `Ctrl+F` / `Shift+F` |
| Rotate Selection 90° | `Alt+F` |

### Tools & View
| Action | Default Shortcut |
|---|---|
| Pencil / Eraser / Fill | `D` / `E` / `F` |
| Eyedropper | Right-Click |
| Selection: Rectangle / Wand / Lasso | `S` / `W` / `L` |
| Tile Tool | `T` |
| Animation Placer | `A` |
| Text Tool | `Q` |
| Toggle Color Picker | `C` |
| Swap FG / BG Colors | `X` *(outside Tile Tool)* |
| Toggle Region Grid | `G` |
| Pan Canvas | `Space` + Left-Drag or Middle-Drag |
| Zoom Canvas | Mouse Wheel |
| Brush Size / Shape / Scatter | `Shift` / `Ctrl` / `Alt` + Mouse Wheel |

### Import & Export
| Action | Default Shortcut |
|---|---|
| Import PNG / GIF | `Ctrl+Alt+E` / `Ctrl+Alt+A` |
| Import Pyxel / Aseprite | `Ctrl+Alt+P` / `Ctrl+Alt+G` |
| Import Image 2 Pixel | `Ctrl+Alt+X` |
| Export Image / Animation | `Ctrl+Shift+E` / `Ctrl+Shift+A` |
| Export Pyxel / Aseprite | `Ctrl+Shift+P` / `Ctrl+Shift+G` |
| Export Selected Region | `Ctrl+Shift+R` |

---

## Troubleshooting & Platform Notes

### Linux
- **AppImage won't execute:** Make sure it has execute permission (`chmod +x pyxross-0.9.0-linux-x86_64.AppImage`). On minimal systems without FUSE2, run `./pyxross-0.9.0-linux-x86_64.AppImage --appimage-extract-and-run` or use the `.tar.gz` archive instead.
- **GNOME Wayland Clipboard:** GNOME Mutter does not implement `wlr-data-control`. In-app copy/paste works normally; pasting images from external apps on GNOME Wayland may fall back to the internal clipboard.
- **No Hardware Vulkan/GPU (VMs / Older Laptops):** Pyxross automatically falls back to OpenGL and software rendering (`llvmpipe`/`lavapipe`).

### Windows
- **Windows SmartScreen Warning:** Portable builds are currently unsigned; click *More info → Run anyway* if you downloaded directly from the official GitHub Releases page and verified its GitHub SHA-256 digest.
- **Keep Folder Structure Intact:** Keep `themes/` and `banners/` in the same folder as `pyxross.exe`.

### Config & Data Locations
- **Linux:** `~/.config/pyxross/` (`workspace.json`, `style.json`, `keybindings.json`, `editor_options.json`, `palettes/`, `banners/`)
- **Windows:** `%APPDATA%\pyxross\`

---

**Previous:** [← 8. Style & Customization](08-style-and-customization.md) | **Back to:** [User Manual Index](README.md)
