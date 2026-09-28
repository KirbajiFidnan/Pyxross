# Pyxross Workshop R10 Release Notes

## Version and Artifacts

The release version is generated from Cargo metadata at build time (`0.1.0`
in the current source tree). Expected artifact names use that metadata:

- `pyxross-<version>-linux-x86_64.tar.gz`
- `pyxross-<version>-linux-x86_64.AppImage`
- `pyxross-<version>-windows-x86_64-portable.zip`

No artifact is claimed here unless it is present in `dist/` and verified by
the release checklist.

## Package Contents

Each package contains the release binary, `README.md`, MIT `LICENSE`, and the
external default theme at `themes/builtin/theme.json` and
`themes/builtin/atlas.png`. The AppImage also includes its desktop entry,
`AppRun`, and `assets/pyxross.png`-derived application icon.

## Build and Checksums

Set a stable non-negative Unix timestamp and run the platform script described
in [R10 release testing](RELEASE_TESTING.md). Verify Linux output with:

```sh
sha256sum -c dist/SHA256SUMS
```

Verify Windows output with:

```powershell
(Get-FileHash .\dist\pyxross-<version>-windows-x86_64-portable.zip -Algorithm SHA256).Hash
Get-Content .\dist\SHA256SUMS-windows
```

## Target Assumptions and Limitations

- Linux is `x86_64-unknown-linux-gnu`; Windows is native
  `x86_64-pc-windows-msvc` only.
- The Windows package is portable only: no signing and no installer.
- There is no signing, CI, upload, or release workflow.
- Linux GUI runtime requires a compatible compositor/display environment.
- Themes are external package resources and must remain beside the binary.

## Verification Status

### Verified in source

- Root-independent staging and package-local resource launch layout (the
  executable-relative theme search is source-verified; native launch remains
  blocked).
- Metadata-derived package/version and approved artifact naming.
- Required resource, checksum, archive-content, and ELF checks in the Linux
  script; optional PE inspection in the Windows script.
- MIT license, desktop entry, package README, and manual smoke check scripts.

### Blocked until native environments are available

- Linux release build and AppImage assembly require a compatible Rust toolchain,
  `x86_64-unknown-linux-gnu`, and externally installed `appimagetool`.
- Windows release build and launch require native Windows, a compatible MSVC
  Rust toolchain,
  and `x86_64-pc-windows-msvc`.
- GUI launch and full adjacent Cargo verification require their respective
  toolchains and display environments.
