# 1. Getting Started & Core Concepts

> **In this chapter:** Creating your first document, understanding what a **Region** is, exploring the workspace layout, and getting a quick overview of how Pyxross organizes game assets.

---

## What Makes a Pyxross Document Different?

Most traditional pixel art editors give you one small canvas per sprite or animation. **Pyxross Workshop** is built specifically for **game asset production** using a **single-canvas, region-based model**:

- Your document is **one unified pixel canvas** (like a sprite sheet or tileset sheet).
- That canvas is divided into a grid of rectangular cells called **Regions** (for example, a `4×4` grid of `16×16` pixel regions = a `64×64` canvas with 16 regions).
- **Tiles** and **Animation Frames** are simply references to regions on this canvas.

### What Is a Region?

A **Region** is a rectangular cell on the canvas (such as `16×16` or `32×32` pixels) that acts as a logical game-asset unit:

- **For Animations:** An animation is a sequence of regions (for example, the 4 cells across row 1 become frames 1–4 of a walk cycle).
- **For Tilesets:** Each cell can hold a linked **Tile** from the Tile Palette — editing a tile updates every copy placed across the canvas.
- **For Exports:** You can export the whole sheet, a single selected region, an animated GIF, or engine-ready sprite sheet frames.

> [!TIP]
> Toggle the region grid overlay at any time by pressing **`G`**.

---

## Creating a New Document

1. Click the **Pyxross flower icon** at the top-left of the Top Bar, or choose **File → New**, or press **`Ctrl+N`**.
2. In the **Create New Document** dialog, configure:
   - **Region Size (px):** `Width` and `Height` of each cell (e.g., `16×16` for retro tiles/sprites, `32×32` for detailed characters). Non-square sizes like `16×24` are supported.
   - **Region Grid:** Number of `Cols` (columns) and `Rows` on the canvas.
   - **Preview:** Shows the live grid layout and total canvas pixel size (e.g., `4×4 grid · 16×16 px/cell` → `64×64 px`).
3. Click **Create**.

A fresh document automatically starts with:
- **1 layer** (`Layer 1`)
- **1 starter animation** (`Animation 1`) mapped to the first 4 regions on the top row
- **A ready-to-use color palette**

You can resize the canvas later via **Canvas → Resize Canvas** (`Alt+R`).

---

## Workspace Overview

Pyxross opens into a dockable dark workspace organized around the central canvas:

| Area | Panels | Purpose |
|---|---|---|
| **Top Bar** | File / Edit / Canvas / Palette / Options menus, New Document icon, FG/BG color swatches, document tabs, pixel-art banner | Quick access to files, actions, active colors, and open projects |
| **Left Dock** | **Toolbox**, **Tool Property** | Select your active tool and adjust its live settings |
| **Center** | **Canvas** | Your unified pixel workspace with region grid, marching-ants selection, and transform gizmos |
| **Right Dock** | **Preview**, **Layers**, **Palette**, **Tile Palette** | Full-canvas preview, layer hierarchy, color swatches, and linked tile management |
| **Bottom Dock** | **Animations**, **Frame Editor**, **Playback** | Manage multiple animations, paint inside a single frame with onion skinning, and watch live playback |

Every panel can be **resized**, **reordered**, or **floated** (click the **Float** button in a panel header).

---

## Quick Overview of the Tools

| Tool | Key | One-Line Summary |
|---|---|---|
| **Pencil** | `D` | Paint crisp pixels, straight lines (`Shift`), and scattered/tapered strokes |
| **Eraser** | `E` | Erase pixels back to transparency with independent brush settings |
| **Fill** | `F` | Flood-fill contiguous or matching-color areas |
| **Eyedropper** | Right-Click | Sample any color directly from the canvas |
| **Selection (Fieldier)** | `S` / `W` / `L` | Select regions via Rectangle (`S`), Magic Wand (`W`), or Lasso (`L`) |
| **Tile Tool** | `T` | Place, flip, rotate, sample, and erase linked tiles |
| **Animation Placer** | `A` | Drag across grid regions to assign frames to the active animation |
| **Text Tool** | `Q` | Stamp crisp pixel-aligned text onto the active layer |

---

## Basic Canvas Navigation

- **Zoom In / Out:** Mouse Wheel over the canvas (anchors around your cursor).
- **Pan Canvas:** Hold **`Space` + Left-Drag** or **Middle-Mouse Drag**.
- **Undo / Redo:** **`Ctrl+Z`** / **`Ctrl+Y`**.
- **Save Project:** **`Ctrl+S`** (or **`Ctrl+Shift+S`** for Save As).

---

**Next:** [2. Toolbox, Tool Properties & All Tools →](02-tools.md)
