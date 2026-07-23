# Licensing and distribution

Ferrink project source is licensed under **GPL-3.0-only**, as declared in the
workspace manifests and supplied in the root [`LICENSE`](../LICENSE). This
document records the distribution policy for this source release; it is not
legal advice.

## Slint-linked embedded binaries

Ferrink uses the pinned Slint snapshot under Slint's GPLv3 option. A distributed
embedded Ferrink binary must therefore be accompanied by its complete
corresponding GPLv3 source, build inputs, lockfile, and all applicable notices.
A proprietary embedded distribution needs an independent Slint commercial
license review.

Primary sources:

- [Slint licensing](https://slint.dev/get-started)
- [Slint pricing and embedded FAQ](https://slint.dev/pricing)
- [Slint terms](https://slint.dev/terms-and-conditions)

## Fonts and optional payloads

The shell uses Inter Variable through `damascene-fonts-inter` 0.1.0. Its
wrapper is `MIT OR Apache-2.0` and the Inter font is `OFL-1.1`; both notices
must travel with a distributed binary.

The optional Fast Atkinson reading-font path retains the Fast-Font MIT notice
and the Atkinson Hyperlegible OFL notice. A generated font payload is not
MIT-only and must retain its font license and attribution.

## Research boundaries

The audited `sh_integration` snapshot did not have a repository-wide license.
Ferrink records externally observable behavior from that audit but does not
copy its implementation text or code. The same rule applies to every
idea-only reference listed in [ACKNOWLEDGMENTS.md](../ACKNOWLEDGMENTS.md).

## Release checklist

- retain `LICENSE`, `LICENSES/`, `Cargo.lock`, and `THIRD_PARTY_NOTICES.md`;
- publish exact source and build scripts for every distributed binary;
- preserve font and copied-code notices;
- run `tools/audit-public-source` and a secret scan; and
- never include device configuration, captured reports, screenshots, document
  data, or application credentials.
