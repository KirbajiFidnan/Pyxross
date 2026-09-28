# R10 Release Testing

## Contract

The package name and version are read from `Cargo.toml` by the packaging
scripts. The approved targets are `x86_64-unknown-linux-gnu` and
`x86_64-pc-windows-msvc`. `SOURCE_DATE_EPOCH` is mandatory. No signing,
installer, CI, upload, or Git workflow is part of R10.

Artifacts are named:

- `pyxross-<version>-linux-x86_64.tar.gz`
- `pyxross-<version>-linux-x86_64.AppImage`
- `pyxross-<version>-windows-x86_64-portable.zip`

## Build

From any working directory, with a compatible Rust toolchain and the target
installed:

```sh
export SOURCE_DATE_EPOCH=1735689600
scripts/package-linux.sh
```

When using rustup, select a specific installed toolchain in the environment,
for example `export RUSTUP_TOOLCHAIN=1.95.0`. Direct Cargo installations ignore
that variable and use their own compatible toolchain.

On native Windows PowerShell with a compatible MSVC Rust toolchain:

```powershell
$env:SOURCE_DATE_EPOCH = '1735689600'
& .\scripts\package-windows.ps1
```

With rustup, select a specific installed toolchain before running the script,
for example `$env:RUSTUP_TOOLCHAIN = '1.95.0'`. Direct Cargo installations use
their own compatible toolchain.

The Linux build additionally requires `appimagetool`. Both scripts stage only
under `dist/`, emit a SHA-256 manifest, and remove their temporary staging
directories after successful output. The Linux `AppRun` resolves its own
directory and changes to the package-local resource root before launching.

## Smoke Checklist

1. Run `tests/manual/package_smoke.sh` after a Linux build, or
   `tests/manual/package_smoke.ps1` after a Windows build.
2. Confirm archive listing contains the binary, `themes/builtin/theme.json`,
   `themes/builtin/atlas.png`, `README.md`, and `LICENSE`; confirm the
   AppImage contains `AppRun`, its desktop entry, executable, external theme,
   atlas, and `assets/pyxross.png`.
3. Confirm checksum verification passes, then deliberately use a corrupted
   copy to confirm archive inspection fails.
4. Extract the archive and launch from a different current directory; the
   package-local theme must still be found. GUI launch requires a compositor.
5. Inspect the AppImage on Linux and launch it outside its directory.
6. On native Windows, launch the portable executable outside its directory.
7. Run adjacent project checks: `cargo test`, `cargo fmt --check`,
   `cargo clippy --locked --all-targets`, and `scripts/check-core-purity.sh`.
8. Exercise theme fallback/switching, palettes, keybindings, persistence, and
   the existing manual UI checklist in `docs/TESTING.md`.

Missing native tools or runtime are blocked environment checks, not passing
results. A smoke script never invents a missing binary or artifact.
