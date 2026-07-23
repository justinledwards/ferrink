# Launcher background sources

The project-authored design sources here do not embed their pixels in the
Ferrink executable:

- `launcher-background-topography-6shade.svg` is the editable spline source;
- `launcher-background-topography-6shade.png` is its compact 632×843 RGB8
  device-ready raster; and
- `two-tone-jacquard-reference.html` records the pixel-field experiment that
  informed the deterministic Rust generator.

No third-party or user-supplied image is shipped in this repository. Users may
place their own licensed images in `/mnt/us/ferrink/backgrounds`; those files
remain local and are not part of Ferrink's distribution.

The built-in pattern is generated in
`crates/ferrink-shell/src/procedural_background.rs`. Optional images use a
deliberately narrow decoder: regular non-symlinked files at most 1 MiB that
decode to exactly 632×843 RGB8 PNG pixels. This is not a general image-loading
surface.
