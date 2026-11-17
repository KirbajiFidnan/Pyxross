# Persistence

> **Status:** D08 implemented (R6 F1+F2, 612 tests green) — directory format + version migration + round-trip (`src/core/document.rs`, `src/io.rs`) and File menu wiring (New/Open/Save/Save As, dirty tracking, unsaved-changes guard). D16 autosave ✔ (R6 F3), D17 recovery journal ✔ (R6 F4) — 5-min autosave to `pyxross/autosave/<project>/` in the config dir, `recovery.json` journal, restore/discard modal at launch (625 tests green). D80 single-file `.pyxross` export package (in-house stored ZIP, openable) — in progress.

## 1. .pyxross Format [D08, D80]

**The directory format is the native working format; a single-file `.pyxross` is an openable export/share package [D80]:**

```
MyProject.pyxross/
├── manifest.json
├── layers/
│   └── <layer_id>/
│       └── <frame_or_page>.png
└── tiles/
    └── <tile_id>.png
```

- **manifest.json:** format version, canvas size, tile size, layer list (id, name, order, visibility, opacity, blend), regions/tiles (rect refs), frames (region ref + delay_ms), animation sequences/tags, palette, editor settings.
- Project palettes are stored in the manifest palette field. Application keybindings are stored separately at the platform config path `pyxross/keybindings.json`; user palettes use `pyxross/palettes/*.json`.
- Palette JSON files placed under a project `palettes/` directory are loaded when the project opens; the manifest palette remains the active project palette.
- Pixel data: per-layer PNGs. Layered data requires more than one flattened PNG — the directory layout supports 4096+ canvases and partial/streaming saves.
- **Tile pixels:** stored as `tiles/<tile_id>.png` (RGBA8, `w × h`), NOT inline in `manifest.json` (v7+). Each tile's `Tile.pixels` is `skip_serializing`, so the manifest stays small even for many tiles; `load_document` refills them from the PNGs. Pre-v7 manifests that still embed `pixels` JSON load unchanged (the embedded array is used as-is).
- Frame/tile pixel content is **not** duplicated in layer buffers beyond the baked tilemap footprints; region/frame references stay rect refs, and tile pixel data lives in the tile palette's PNGs.

### Single-file `.pyxross` container [D80]

- A single-file `.pyxross` is an **in-house stored ZIP** (uncompressed entries, CRC32 only — no compression, no new dependency). It is an **openable export/share package**; the directory format above is the native working format and stays the autosave/recovery format (D16).
- **Deterministic:** entries are emitted in a fixed order with fixed metadata (no timestamps or other variable fields), so the same document always packs to a byte-identical archive.
- **Layout:** identical to the directory tree — `manifest.json` + `layers/<layer_id>.png` + `tiles/<tile_id>.png`. The PNGs are written **first** and `manifest.json` **last**, so an interrupted write leaves an archive with no readable manifest and is rejected rather than half-loaded.
- `FORMAT_VERSION` **stays v8** — the container is not a schema change; the manifest format is unchanged.
- **Type detection:** `is_dir()` → the directory loader; a regular file whose bytes start with the ZIP magic (`PK\x03\x04`) → the archive loader. `load_document` accepts both.
- **Security:** entries are **never extracted to disk** — the loader looks up only the expected names and ignores unknown or `../`-containing entries (zip-bomb / path-traversal rejection); archives larger than **4 GiB** are rejected.
- **Saving:** in-place `tmp` + `rename` (atomic), falling back to target-delete-then-rename on Windows; **Save As** is how a user converts between directory and single-file, never a silent conversion on Save.

### Round-trip requirement
save → load → compare must be lossless (unit-tested in core).

## 2. Autosave [D16]

- Interval: **5 minutes** default (configurable) → writes to user config dir `autosave/<project>/`.
- Never writes over the user's file directly; autosave is a separate copy.

## 3. Crash Recovery

- A small **recovery journal** tracks save state; on launch, if a recovery exists, the user is prompted: restore or discard.
- Recovery restores: last autosave + journal metadata.

## 4. Export Suite [D21]

| Target | Output |
|---|---|
| Full canvas | single PNG |
| Per-tile | one PNG per tile region |
| Sprite sheet | atlas PNG + JSON metadata (rects, frame durations, tags) |
| Animation | GIF |
| **Export Selected** | active region → PNG, **composite of all visible layers** (visibility/opacity/blend respected) [MVP must-have] |

## 5. Import [D20]

- v1: PNG → new canvas or new region source.
- Post-MVP order: Aseprite (`.ase/.aseprite`) → Pyxel Edit (`.pyxel`); import maps layers/cels/frames/tags/palette onto our model.
- OS clipboard PNG import also lands pixels (D14).

## 6. Versioning, Errors & Migration

- `manifest.json` carries `format_version`; loaders migrate older versions forward.
- **Dual acceptance [D80]:** `load_document` accepts **both** a directory and a single-file `.pyxross` archive (type detection in §1). Directory projects from any earlier build remain readable indefinitely — adding the single-file container does not change the directory loader.
- **Corrupt / CRC errors [D80]:** a bad CRC on any expected entry, a missing `manifest.json`, or a manifest that fails to parse makes the **whole archive fail to load with an error**; it is never partially applied. Because entries are never extracted to disk, a malicious archive cannot write outside the project.
- **Old build, new file [D80]:** a build that predates D80 sees a single-file `.pyxross` as an unrecognized path (not a directory) and reports that it cannot open it — no silent data loss. The directory format is unchanged, so old builds keep opening directory projects normally.
- Binary single-file export format (a packed/compressed pixel payload): still deferred after v1; D80's stored-ZIP container is an export/share package, not that binary format.
