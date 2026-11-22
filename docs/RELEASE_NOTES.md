# Pyxross Workshop v0.9.1 Release Notes

## Downloads

Prebuilt portable binaries for **v0.9.1**:

- **Linux AppImage (x86_64, GLIBC 2.35+):** `pyxross-0.9.1-linux-x86_64.AppImage`
- **Linux Portable Archive (x86_64, GLIBC 2.35+):** `pyxross-0.9.1-linux-x86_64.tar.gz`
- **Windows Portable ZIP (x86_64, Windows 10/11):** `pyxross-0.9.1-windows-x86_64-portable.zip`

## What's New in 0.9.1

- **File Manager:** Sortable `Name`, `Size`, and `Modified` columns (ascending/descending), live autocomplete search in `File name` (including extension matching like `.png` with `Tab` completion), and a `File type` filter dropdown.
- **Image-to-Pixel:** Smooth pointer-centered preview zoom & pan with `Fit` reset, plus `Save Preset` and `Load Preset` (`.json`) controls in the `Size` tab.
- **Import Banner Panel:** New two-column banner importer launched from `Select Banner` with `Banner Name` input, `Cut (8:1)` / `Full Image` modes, zoomable/pannable preview, draggable 8:1 crop frame with shaded exterior, and direct PNG export into `banners/`.
- **Top-Bar Banner Shortcut:** Clickable hotspot on the top-bar banner region with a subtle `"Click to change banner"` hover hint.
- **Style Panel:** Ergonomic `Save` (enabled when customized) and `Load` theme dialog workflow plus a `Spacing` slider.
- **Key Bindings Panel:** Redesigned with live search, category filter tabs (`Tools`, `Edit`, `Transform`, `File`, `Import / Export`, `View`), click-to-record monospace keycap badges, inline conflict indicators, manual key/modifier popover, and a pinned `Reset` / `Apply` footer.

## Fixes

- **Windows Input & Render Latency:** Configured swapchain `desired_maximum_frame_latency = 1`, added `window.pre_present_notify()`, and flushed canvas texture uploads in the same frame after input handling to eliminate multi-frame cursor lag.
- **Overlapping Panel Z-Order:** Clicking the front floating panel (or foreground popups) no longer raises occluded background panels or triggers background header drags.
- **Fast Canvas Edge Strokes:** Rapid inward and outward brush swipes now interpolate cleanly across canvas borders without missing edge pixels while staying paused during off-canvas motion.
- **Panel Layout & Scrollbars:** Fixed narrow-panel slider/button overlap in `Style`, pinned top action buttons in resizable `Select Banner`, eliminated resize scrollbar flicker and text overflow in `File Manager`, and kept `Key Bindings` footer buttons clear of scrollbars.
- **Pencil Default Shortcut:** Guarded test keymap persistence so unit tests cannot overwrite user `keybindings.json` (keeping Pencil on `D`).

## Package Contents

Each package contains:
- Release binary (`pyxross` or `pyxross.exe`)
- `README.md`, MIT `LICENSE`, and `LICENSE-FONT-OFL.txt` (SIL OFL 1.1 for embedded JetBrains Mono Nerd Font)
- External default theme at `themes/builtin/theme.json` and `themes/builtin/atlas.png`
- Built-in top-bar banners in `banners/`
- Linux AppImage additionally includes `AppRun`, `pyxross.desktop`, and `pyxross.png`

## Platform Notes

- Linux builds target `x86_64-unknown-linux-gnu` on an Ubuntu 22.04 (`GLIBC 2.35`) baseline; Windows builds target native `x86_64-pc-windows-msvc`.
- Windows builds are portable (unsigned, no installer).
- On GNOME Wayland, OS-level image clipboard interop with external apps is limited because GNOME Mutter does not implement `wlr-data-control`; in-app copy/paste works normally.
