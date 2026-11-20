# Pyxross Workshop

**Pyxross Workshop** is a native, cross-platform, region-based pixel art, tileset, and sprite animation editor written in Rust (`winit` + `wgpu` + `egui`).

Inspired by **Pyxel Edit** and **Aseprite**, Pyxross treats the canvas as a unified sprite sheet where rectangular regions become reusable tiles and animation frames — with layered editing, pixel-perfect rotation (**RotSprite** / **CleanEdge** / **Rotxel**), dockable panels, and single-file `.pyxross` project archives.

---

## Key Features

- **Region-Based Workflow:** Organize tilesets and sprite animations directly on one canvas using regions and linked tile instances.
- **Pixel-Perfect Transforms:** Free rotation and scaling with RotSprite, CleanEdge, and Rotxel algorithms, plus exact 90°/180°/270° and flip operations.
- **Layers & Groups:** Nested layer groups, opacity, visibility, locking, and non-destructive undo/redo history.
- **Animation Suite:** Multiple animations per document, per-frame timing & speed multiplier, live preview panel, frame editor with onion skinning, and sprite sheet / GIF export.
- **Importers & Exporters:**
  - Native `.pyxross` single-file ZIP archive (and directory projects) with autosave & crash recovery.
  - **Aseprite** (`.ase` / `.aseprite`) import & export.
  - **Pyxel Edit** (`.pyxel`) import & export.
  - **PNG**, **GIF**, and **Image-to-Pixel** conversion (sampling, K-means palette extraction, dithering).
- **Customizable UI:** Dockable/floating panels, custom 9-slice themes (`themes/user` or `PYXROSS_THEMES`), top-bar pixel-art banners, and remappable keybindings.
- **Built-in File Manager:** In-app file browser with OS standard folders, Windows drive letters, and Linux GTK/KDE bookmarks — works reliably across all desktop environments without external portal dependencies.

---

## Supported Platforms

### Linux (`x86_64`)
- Built against **GLIBC 2.35** (Ubuntu 22.04 LTS baseline), compatible with Ubuntu 22.04+, Debian 12+, Linux Mint 21+, Pop!_OS, Fedora, openSUSE, SteamOS, and Arch Linux.
- **Display Servers & Desktop Environments:** Native **Wayland** and **X11** support (Hyprland, Sway, River, KDE Plasma, GNOME, XFCE, Cinnamon, MATE, i3, Openbox).
- **GPU Backends:** Vulkan and OpenGL, with automatic software fallback (`llvmpipe`/`lavapipe`) when hardware acceleration is unavailable.
- **Known DE Note (GNOME Wayland):** GNOME Mutter does not implement `wlr-data-control`. Internal copy/paste always works; OS-level image copy/paste with external apps on GNOME Wayland may fall back to the internal clipboard.

### Windows (`x86_64`)
- Native **Windows 10 / 11** portable executable (`x86_64-pc-windows-msvc`).
- **GPU Backends:** DirectX 12, Vulkan, and OpenGL, with automatic software fallback (WARP).
- Portable ZIP package (no installer or system modifications required).

---

## Running Prebuilt Packages

### Linux (AppImage)
```sh
chmod +x pyxross-0.9.0-linux-x86_64.AppImage
./pyxross-0.9.0-linux-x86_64.AppImage
```

### Linux (Portable Archive)
```sh
tar -xzf pyxross-0.9.0-linux-x86_64.tar.gz
cd pyxross-0.9.0-linux-x86_64
./pyxross
```

Verify Linux checksums:
```sh
sha256sum -c SHA256SUMS
```

### Windows (Portable ZIP)
1. Extract `pyxross-0.9.0-windows-x86_64-portable.zip` to any folder.
2. Keep `themes/` and `banners/` alongside `pyxross.exe`.
3. Launch `pyxross.exe`.

---

## Building from Source

Requires **Rust 1.95+** (`rust-version = "1.95"`).

```sh
cargo build --release
./target/release/pyxross
```

### Packaging Scripts
- **Portable Linux (Docker, GLIBC 2.35):** `./scripts/package-linux-portable.sh`
- **Native Linux:** `SOURCE_DATE_EPOCH=$(date +%s) ./scripts/package-linux.sh`
- **Native Windows (PowerShell):** `$env:SOURCE_DATE_EPOCH = '1735689600'; .\scripts\package-windows.ps1`

---

## License & Acknowledgments

- **Pyxross Workshop** is licensed under the [MIT License](LICENSE).
- Embedded UI icon font: **JetBrains Mono Nerd Font**, licensed under the [SIL Open Font License 1.1](assets/fonts/OFL.txt).
- Built with `egui`, `egui_dock`, `wgpu`, `winit`, `image`, `png`, `gif`, `arboard`, `serde`, and `flate2`.
- Image-to-Pixel pipeline inspired by **SLK_img2pixel** (CC0-1.0) by Lukas Holzbeierlein.
