# 5. Tile Palette & Linked Tiles

> **In this chapter:** How Pyxross's linked tile system works, managing tiles in the Tile Palette, placing/flipping/rotating tiles on the canvas, and live-editing tiles across an entire map.

---

## What Are Linked Tiles?

When building a 2D game level or tileset (grass, brick walls, water, dungeon floors), you often reuse the same `16×16` or `32×32` tile dozens of times.

In Pyxross, **Tiles are linked instances**:
- Each unique tile lives in your **Tile Palette** (bottom-right dock).
- Placing a tile into grid cells stores a **reference** (plus optional flip/rotation) to that tile.
- **Live Sync:** Whenever you paint, erase, fill, or transform pixels inside **any** placed copy of a tile, **every other copy of that tile across the canvas updates automatically** (including flipped and rotated copies, transformed back cleanly)!

---

## Tile Palette Panel

Located in the bottom-right dock:

| Control | Function |
|---|---|
| **`+ Empty`** | Adds a new blank tile sized to the document's Region Size (`tile_size`) |
| **`- Delete`** | Deletes the currently selected tile from the palette |
| **`↑` / `↓`** | Reorders tiles in the Tile Palette |
| **Tile Thumbnails** | Click any tile thumbnail to select it for placement with the **Tile Tool (`T`)** |

---

## Using the Tile Tool (`T`)

Press **`T`** (or select the Tile icon in the Toolbox) to enter Tile Placement mode. Hovering over the canvas shows a live **ghost preview** of the tile snapped to the region grid.

### Placement Controls
| Action | Input |
|---|---|
| **Place single tile / drag-paint tiles** | Left-Click / Left-Drag |
| **Fill rectangular area with active tile** | **`Shift` + Left-Drag** |
| **Erase tile from cell(s)** | Right-Click / Right-Drag |
| **Erase rectangular area of tiles** | **`Shift` + Right-Drag** |
| **Sample tile from canvas (Tile Eyedropper)** | **`Alt` + Click** on a placed tile |

### Placement Orientation Modifiers (While Tile Tool Is Active)
| Key | Effect on Next Placed Tile |
|---|---|
| **`X`** | Flip tile horizontally (note: outside the Tile Tool, `X` swaps FG/BG colors) |
| **`V`** | Flip tile vertically |
| **`R`** | Rotate tile 90° clockwise |
| **`Shift+R`** | Rotate tile 90° counter-clockwise |

---

## Editing a Placed Tile Live

1. Place a tile across several cells on your canvas using the **Tile Tool (`T`)** (try flipping a few copies with **`X`** to test seamlessness!).
2. Switch to **Pencil (`D`)**, **Eraser (`E`)**, or **Fill (`F`)**.
3. Paint directly inside any one of those cells on the canvas.
4. Watch every linked copy update in real time — and undo/redo (`Ctrl+Z` / `Ctrl+Y`) cleanly reverts all linked instances together!

> [!TIP]
> **Pyxel Edit Compatibility:** Pyxross imports and exports `.pyxel` tilemaps, tile palettes, and flipped/rotated tile orientations pixel-for-pixel!

---

**Previous:** [← 4. Animation System](04-animations.md) | **Next:** [6. Colors, Palettes & Color Picker →](06-color-and-palette.md)
