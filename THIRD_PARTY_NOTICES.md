# Third-party notices

Ferrink's own source is GPL-3.0-only; see [LICENSE](LICENSE). This file is a
plain-language map to the other licenses that travel with the source. It is not
legal advice.

## Slint

Ferrink pins Slint from the [Slint project](https://github.com/slint-ui/slint)
at commit `3c5e0a606d8843fabaf00c5fbbde4fd5be08ba4f`. Ferrink uses Slint under
its GPLv3 option. Anyone distributing a Ferrink binary must also provide the
corresponding source, lockfile, build instructions, and required notices under
the GPL's terms. A different distribution model needs its own Slint license
review.

## Fonts

- `damascene-fonts-inter` supplies the Inter wrapper under `MIT OR Apache-2.0`;
  the Inter font itself is [OFL-1.1](https://openfontlicense.org/). The license
  texts are in [LICENSES](LICENSES).
- The optional reading-font helper retains its complete Fast-Font MIT notice and
  Atkinson Hyperlegible OFL-1.1 notice in
  [`apps/org.koreader.reader/font-assets`](apps/org.koreader.reader/font-assets).

## Rust dependencies

`Cargo.lock` is the exact dependency lock for this checkout. For convenience,
[LICENSES/DEPENDENCY_LICENSES.tsv](LICENSES/DEPENDENCY_LICENSES.tsv) is a
machine-generated inventory from `cargo metadata --locked`; it lists package,
version, source, declared license expression, and repository URL. Individual
dependency license texts remain with their upstream source packages.

## Ideas are not copied code

The projects credited in [ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md) informed
research or visual direction unless called out above. Ferrink does not copy
their code, tests, prose, icons, images, or packaged assets into this source
release. In particular, `KindleModding/sh_integration` had no identified
repository-wide license during review, so it is treated as behavioral research
only.
