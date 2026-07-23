# Fast Atkinson font payload

Ferrink does not vendor the generated font binaries in Git. The staging script
accepts a checkout of `Born2Root/Fast-Font` at the pinned revision, verifies all
four faces byte-for-byte, and produces the additive KOReader font directory.

- Source: <https://github.com/Born2Root/Fast-Font>
- Revision: `aeae0775d9251365eae3b133cbf26ce0366f6108`
- Base font: Atkinson Hyperlegible by Braille Institute of America
- Base source: <https://github.com/googlefonts/atkinson-hyperlegible>
- Installed family name: `Fast Atkinson Hyperlegible`
- Destination: `/mnt/us/koreader/fonts/ferrink-fast-atkinson/`

The generated font remains under OFL-1.1. `Fast-Font-MIT.txt` preserves the
license notice for the feature-generation work. The font payload must include
both notices and `SHA256SUMS`.
