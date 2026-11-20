# 3. Layers & Layer Groups

> **In this chapter:** How Pyxross's layer stack works, creating and nesting layer groups, adjusting opacity and blend modes, locking, and merging layers.

---

## Understanding the Layer Stack

The **Layers** panel (right dock) manages all layers in the active document from top (foreground) to bottom (background). Every drawing, fill, selection, tile, and text operation applies to the **currently selected (highlighted) layer**.

### Top Action Bar
| Button | Action |
|---|---|
| **`+ Add`** | Creates a new empty transparent layer above the active layer |
| **`Delete`** | Removes the active layer (disabled if only one layer remains) |
| **`Merge`** | Merges the active layer down into the layer below it |
| **`New group`** | Creates a collapsible folder group to organize related layers |

---

## Layer Controls (Per Row)

Each row in the Layers panel provides quick controls:

- **Visibility (Eye icon):** Toggle whether the layer is visible in the canvas, preview, playback, and exports.
- **Layer Name:** Click a row to make it the active editing layer.
- **Group Collapse/Expand:** Group rows show a toggle arrow to fold/unfold their child layers cleanly.
- **Lock Indicator:** Locked layers prevent accidental painting, erasing, or transforms.
- **Settings (Gear icon):** Opens the floating **Layer Settings** panel for that specific layer.

---

## Layer Settings Panel

Clicking the gear icon on any layer opens its **Layer Settings**:

- **Name:** Rename the layer or group (e.g., `Outline`, `Base Colors`, `Shading`, `FX`).
- **Opacity (`0%`–`100%`):** Controls how transparent the layer is when composited.
- **Blend Mode:** Controls how the layer blends with layers beneath it:
  - **`Normal`** — Standard alpha compositing.
  - **`Multiply`** — Darkens underlying colors (great for shadows).
  - **`Screen`** — Brightens underlying colors (great for glows and lighting).
  - **`Add`** — Additive brightness (great for fire, lasers, and magic FX).
- **Ungroup** *(shown on group layers only)*: Dissolves the group folder while keeping all child layers intact.

---

## Organizing With Layer Groups

For complex game sprites and tilesets, group layers let you separate parts cleanly:

```text
▾ Character (Group)
    ├── Highlights (Add, 60%)
    ├── Shading (Multiply, 45%)
    ├── Base Colors (Normal, 100%)
    └── Outline (Normal, 100%)
▾ Background Tiles (Group)
    └── Ground Layer
```

- **Toggling a Group's Eye Icon** hides or shows all layers inside it at once.
- **Reordering:** Drag or move layers to adjust stacking order and group membership.
- **Interoperability:** Nested layer groups, visibility, opacity, and lock flags are preserved when exporting/importing **`.pyxross`**, **Aseprite (`.ase`/`.aseprite`)**, and **Pyxel Edit (`.pyxel`)** files!

> [!TIP]
> Keep your **Tile Layer** separate from freehand **Detail / Overlay Layers** so you can paint reusable tiles without accidentally overwriting custom per-cell decorations.

---

**Previous:** [← 2. Toolbox & Tools](02-tools.md) | **Next:** [4. Animation System →](04-animations.md)
