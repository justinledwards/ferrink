# Upstream research boundaries

Ferrink uses upstream projects as evidence and design input, not as a source
of uncredited code or assets. Exact source pins are listed in
[ACKNOWLEDGMENTS.md](../ACKNOWLEDGMENTS.md).

## Findings retained

- Device identity, framebuffer ABI, input devices, front light, power, USB,
  and stock-process behavior vary substantially across hardware and firmware.
  A public tool must probe and reject unknown combinations rather than infer
  compatibility from a product name or resolution.
- Direct framebuffer programs need deterministic ownership, bounded I/O,
  explicit input-release handling, and a stock-recovery plan. A foreground app
  is not automatically an early-boot replacement.
- Small E Ink UIs benefit from compact status chrome, high-contrast controls,
  large touch targets, restrained spacing, and low-refresh-cost interaction.
- A time-aware literary display can be a local optional feature, but a corpus
  requires its own provenance and license review. Ferrink ships no third-party
  excerpts or corpus.
- Diagnostic bundles must exclude credentials, network data, document paths,
  raw input, screenshots, and unique device identifiers.

## Code and asset boundary

Ferrink is independently implemented in Rust and Slint. It does not copy code,
tests, fixtures, prose, icons, or artwork from the idea-only references.
Slint and the font-related notices are direct dependencies or staged optional
assets; their terms are recorded in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

The audited `KindleModding/sh_integration` snapshot did not identify a
repository-wide license. Ferrink therefore uses it only as behavioral research
and never copies implementation text or code from it.

## Revalidation rule

Any future physical-device work must begin with a locally held, reviewed
profile and fresh local evidence. Nothing in this public repository authorizes
a device operation or asserts support for a specific device or firmware.
