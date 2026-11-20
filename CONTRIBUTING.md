# Contributing to Pyxross Workshop

Thank you for your interest in contributing to **Pyxross Workshop**! Whether you are reporting a bug, testing on a new Linux desktop environment or Windows machine, improving documentation, or submitting a pull request, every bit of help is appreciated.

## Ways to Help

- **Test & Report Bugs:** Try Pyxross on your OS / desktop environment / GPU and open a [Bug Report](https://github.com/KirbajiFidnan/Pyxross/issues/new?template=bug_report.yml) with your environment details.
- **Suggest Features:** Share game-asset workflow ideas via [Feature Request](https://github.com/KirbajiFidnan/Pyxross/issues/new?template=feature_request.yml).
- **Code & Themes:** Fix bugs, optimize rendering/codecs, or share custom themes.

## Development Setup

Requires **Rust 1.95+** (`rust-version = "1.95"`).

```sh
git clone https://github.com/KirbajiFidnan/Pyxross.git
cd Pyxross
cargo run
```

## Architecture Rules

1. **Core Purity:** `src/core` must never depend on `egui`, `wgpu`, `winit`, or OS/UI crates. Verify with:
   ```sh
   ./scripts/check-core-purity.sh
   ```
2. **Theme System:** Route UI chrome colors through `ThemeColors` (`src/ui/theme.rs`), not hard-coded colors.
3. **Tests:** Run the full test suite before opening a pull request:
   ```sh
   cargo test
   ```

## License

By contributing to Pyxross Workshop, you agree that your contributions will be licensed under the [MIT License](LICENSE).
