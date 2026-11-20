# Pyxross Workshop v0.9.0 Release Notes

## Version and Artifacts

The release version is generated from `Cargo.toml` metadata (`0.9.0`). Expected artifact names:

- `pyxross-0.9.0-linux-x86_64.tar.gz`
- `pyxross-0.9.0-linux-x86_64.AppImage`
- `pyxross-0.9.0-windows-x86_64-portable.zip`

## Highlights in 0.9.0

- Dockable & floating panel workspace (`egui_dock`) with customizable layout, top bar, and pixel-art banners.
- Single-file `.pyxross` archive format with atomic save and crash recovery.
- Aseprite (`.ase` / `.aseprite`) and Pyxel Edit (`.pyxel`) import and export.
- Animation dock suite (preview, frame editor with onion skinning, GIF & sprite sheet export).
- Image-to-Pixel converter (sampling, K-means palette extraction, dithering).
- Built-in cross-platform File Manager supporting Windows drive letters, Linux GTK2/3 bookmarks, and KDE Dolphin XBEL bookmarks.
- Cross-platform hardening: GLIBC 2.35 portable Linux builds, swapchain `Outdated`/`Lost` reconfiguration, minimize (`0x0`) guard, software GPU adapter fallback, native window icon and Wayland `app_id` / X11 `WM_CLASS`, and backup-protected Windows file replacement.

## Package Contents

Each package contains:
- Release binary (`pyxross` or `pyxross.exe`)
- `README.md`, MIT `LICENSE`, and `LICENSE-FONT-OFL.txt` (SIL OFL 1.1 for embedded JetBrains Mono Nerd Font)
- External default theme at `themes/builtin/theme.json` and `themes/builtin/atlas.png`
- Built-in top-bar banners in `banners/`
- Linux AppImage additionally includes `AppRun`, `pyxross.desktop`, and `pyxross.png`

## Build and Checksums

### Linux (Portable GLIBC 2.35 via Docker)
```sh
./scripts/package-linux-portable.sh
```

Verify Linux output from `dist/`:
```sh
(cd dist && sha256sum -c SHA256SUMS)
```

### Windows (Native PowerShell, MSVC)
```powershell
$env:SOURCE_DATE_EPOCH = '1735689600'
& .\scripts\package-windows.ps1
```

Verify Windows output:
```powershell
(Get-FileHash .\dist\pyxross-0.9.0-windows-x86_64-portable.zip -Algorithm SHA256).Hash
Get-Content .\dist\SHA256SUMS-windows
```

## Target Assumptions and Limitations

- Linux target is `x86_64-unknown-linux-gnu` (built on Ubuntu 22.04 / GLIBC 2.35 baseline); Windows target is native `x86_64-pc-windows-msvc`.
- Windows package is portable only (unsigned, no installer).
- On GNOME Wayland, OS-level image clipboard interop with external apps is limited because GNOME Mutter does not implement `wlr-data-control`; in-app copy/paste works normally.
- Themes remain external package resources beside the binary (`themes/builtin/`).
