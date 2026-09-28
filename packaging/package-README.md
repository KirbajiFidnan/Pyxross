# Pyxross Workshop portable package

This is a portable package for Pyxross Workshop. Start `pyxross` from this
directory (or use the AppImage). The launcher keeps the package-local resource
root as the working directory, so launch location does not affect resources.

The package includes the external default theme at
`themes/builtin/theme.json` and `themes/builtin/atlas.png`. User themes may be
placed in `themes/user` or supplied through `PYXROSS_THEMES`.

This package contains no installer, signing, update service, or upload step.
