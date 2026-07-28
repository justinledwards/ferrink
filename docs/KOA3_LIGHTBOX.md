# KOA3 lightbox and halftone display layer

## Status

This note records a physically verified KOA3 display mechanism discovered on
2026-07-28. Ferrink now implements it for the exact reviewed KOA3 profile. The
shell owns a two-phase apply/repaint/commit sequence, clears explicitly before
normal handoff, and the out-of-process guardian independently clears it before
stock crash recovery. Host tests and the reviewed ARM `cargo zigbuild` pass;
the integrated drawer still requires the physical-panel acceptance described
below because no screenshot path can show this layer.

The mechanism is useful for Ferrink's quick-settings drawer: the drawer can
remain sharp while the home surface below it is rendered with Amazon's subtle
checkerboard defocus treatment. It does not require Ferrink to render a bitmap
checkerboard into the framebuffer, link to Amazon's native library, fork
Awesome, or add an X11 dependency.

## Why screenshots are not evidence

The checkerboard is applied after ordinary framebuffer pixels. While the stock
quick-settings drawer was physically visible, Amazon's postprocessed
`/usr/sbin/screenshot -f` output omitted the pattern. Raw framebuffer capture
also precedes physical panel processing, so neither capture path can establish
that this layer is active. The same mismatch occurred after a foreground
handoff: Ferrink's pixels looked normal in the capture while the physical panel
retained the checkerboard below the former stock drawer.

Consequences:

- a fresh screenshot remains useful for layout and safe touch targeting;
- it cannot prove that the lightbox is active or cleared;
- a live physical observation is required for the mechanism gate; and
- Ferrink must track the state it requested instead of trying to detect the
  pattern by reading framebuffer pixels.

The stock X window tree provides a presentation preflight, not a pixel test. A
mapped stock lightbox window contains both `LB:ON` and
`MapState=IsViewable`. Ferrink already declines foreground acquisition in that
state so it cannot inherit an open stock drawer or modal.

## Stock control path

The reviewed firmware implements lightbox policy in
`/etc/xdg/awesome/lab126_ligl.lua`. Windows opt in through the `LB:ON` metadata
field. The policy records the visible dialog and optional keyboard rectangles,
then calls Amazon's native LIGL layer:

```text
X window with LB:ON
  -> Amazon Awesome Lua lightbox policy
  -> ligl.apply_halftone(mode, dialog[, keyboard])
  -> liblab126IGL.so.1.0
  -> ioctl(fd, 0x4024464b, 36-byte region request)
  -> full Zelda-88 GC16 repaint
  -> physical EPDC output
```

`com.lab126.winmgr lightboxMode` is a read/write policy property. Changing it
updates the mode used by the Lua policy; it does not apply or clear a region by
itself. Mode `1` is the reviewed stock default. Other nonzero pattern modes and
mode `0` were visible in the policy source but were not characterized and must
not be inferred to work.

The stock implementation sends the legacy lightbox ioctl through the same
`/dev/fb0` descriptor used for display updates. No separate kernel device is
involved.

## Exact request layout

The request number is `0x4024464b`: a 36-byte write request with ioctl type
`0x46` and command `0x4b`. Disassembly of the reviewed
`ligl_display_eink_v2_apply_halftone` implementation and live register capture
established the argument translation.

The public native call accepts:

```text
mode, x1, y1, width1, height1, x2, y2, width2, height2
```

The kernel payload is nine native-endian 32-bit words in this order:

```text
top1, left1, width1, height1,
top2, left2, width2, height2,
mode
```

The stock KOA3 quick-settings drawer is a single sharp foreground rectangle.
Its captured public arguments were:

```text
mode=1, x=0, y=0, width=1264, height=1260
```

The corresponding kernel words are:

```text
[0, 0, 1264, 1260, 0, 0, 0, 0, 1]
```

The stock clear call keeps mode `1` and supplies no regions:

```text
[0, 0, 0, 0, 0, 0, 0, 0, 1]
```

The named rectangles are the areas that remain sharp. The display-processing
layer defocuses the area outside their union. This is the right shape for a
top drawer: submit the drawer bounds, not the bounds of the content that should
be checkerboarded.

## The KOA3 `EINVAL` contract

On this KOA3 kernel, the lightbox ioctl returns `EINVAL` for Amazon's exact
known-good apply and clear calls. Amazon deliberately continues after that
return and submits a full display update. The physical checkerboard still
changes as requested.

A bounded direct experiment reproduced that behavior without using the stock
drawer:

1. the stock quick-settings window was closed and the X inventory confirmed no
   mapped `LB:ON` window;
2. a static ARM helper sent the exact zero-region clear request;
3. it sent the exact stock quick-settings region request;
4. it accepted only the observed `EINVAL` result and invoked the reviewed stock
   repaint;
5. the operator physically observed the checkerboard below the supplied
   1260-row foreground region;
6. after 15 seconds the helper sent the zero-region clear request and repainted;
   and
7. the operator physically observed the checkerboard disappear.

The helper passed host Clippy and a static ARMv7 musl `cargo zigbuild`; its
staged checksum matched before execution. It did not map, read, or write
framebuffer pixels. Experimental binaries and live evidence stayed under
ignored local/device paths and are not repository artifacts.

This is a narrow exception, not a general error policy. Production code may
treat `EINVAL` as the observed lightbox result only when all of the following
match the reviewed KOA3 profile:

- exact request number and 36-byte layout;
- exact framebuffer identity and geometry;
- validated in-bounds foreground regions;
- mode `1`;
- an immediately following reviewed display refresh; and
- an explicit clear-and-refresh path.

Every other ioctl error remains a failure. The fact that a rejected request has
an observable side effect is precisely why cleanup cannot be implicit.

## Required repaint

The legacy lightbox request changes display-processing state; it does not make
that state visible by itself. Stock follows it with a full-screen Zelda-88
GC16 update. The captured stock request contained:

| Field | Value |
| --- | --- |
| Region | top `0`, left `0`, width `1264`, height `1680` |
| Waveform | `2` (`GC16`) |
| Update mode | `1` (full) |
| Marker | changing nonzero value |
| Temperature | `0x1000` (ambient) |
| Flags | `0` |
| Dither / quantization | `0` / `0` |
| Alternate buffer | all zero |
| Histogram waveform modes | black/white `1`, grayscale `2` |

During characterization, `/usr/bin/xrefresh -display :0.0` caused stock LIGL
to issue this repaint. That was a convenient way to prove the independent
lightbox request while stock owned the foreground. Ferrink uses its existing
typed Zelda display adapter and adds neither X11 nor `xrefresh` as a launcher
dependency. It forces one full update for each changed lightbox state. The
adapter's reviewed full-update encoder uses zero histogram waveform fields, so
the integrated physical acceptance specifically checks that this produces the
same apply and clear result on the panel.

## Ferrink lifecycle contract

Lightbox state belongs to the display adapter, not to a Slint callback and not
to shell business logic. Slint may emit only a drawer-open or drawer-close
intent. A typed KOA3 adapter owns validation, ioctl submission, refresh
ordering, and state tracking.

The safe open sequence is:

```text
validate exact KOA3 profile and drawer bounds
  -> submit one mode-1 foreground-region request
  -> accept only success or the reviewed KOA3 EINVAL
  -> render the open drawer
  -> submit one reviewed full refresh
  -> mark lightbox active only after refresh submission succeeds
```

The safe close sequence is:

```text
submit one mode-1 zero-region clear request
  -> accept only success or the reviewed KOA3 EINVAL
  -> render the closed home surface
  -> submit one reviewed full refresh
  -> mark lightbox inactive only after refresh submission succeeds
```

Additional rules:

- never resend the region for ordinary dirty updates while the drawer remains
  open;
- never use the stock X window inventory as Ferrink's own active-state flag;
- clear before launching an application, returning to stock, releasing the
  foreground lease, suspending, or changing display mode;
- make close idempotent so a second explicit close performs no ioctl;
- do not send an ioctl from `Drop`;
- on a normal shutdown, clear through an explicit fallible close method;
- on a shell crash, the out-of-process guardian must clear the reviewed state
  before its promoted stock repaint; and
- if the guardian cannot establish the exact profile, descriptor, request, and
  repaint contract, use the existing stock recovery path rather than guessing.

An apply failure before refresh leaves the drawer closed. A repaint failure
after the apply request requires an immediate explicit clear attempt followed
by foreground recovery. A clear or clear-repaint failure is a recovery fault,
not a cosmetic warning.

## Verification coverage

Host coverage includes:

- compile-time request size, alignment, field offsets, and ioctl number;
- public `(x, y)` to kernel `(top, left)` ordering;
- nonempty, overflow, and out-of-bounds foreground validation through the
  existing typed refresh-region boundary;
- exact apply and clear payloads;
- the narrow KOA3 `EINVAL` acceptance rule;
- no duplicate apply/clear calls across repeated UI events;
- two-phase apply/clear state that cannot commit before presentation;
- explicit cleanup on normal close and application handoff;
- guardian cleanup after a simulated shell crash; and
- no lightbox authority on PW1 or an unknown profile.

The shell integration test also drives the production Slint callbacks through
physical KOA3 coordinates and verifies that opening and closing the drawer emit
the exact 1264-by-732 sharp foreground intent.

The remaining device gate uses a freshly built ARM artifact, a closed stock
lightbox preflight, a physically observed drawer apply and clear, and a final
healthy application or stock handoff. Screenshots alone cannot satisfy it.

## Investigated alternatives

- **Repeated GC16 without a clear request:** does not clear the inherited
  lightbox state. The state and the repaint are separate parts of the contract.
- **Framebuffer pixel detection:** cannot observe the physical checkerboard and
  risks confusing document pixels with display processing.
- **`awesome-client`:** the installed client had no usable session D-Bus path,
  and Ferrink does not need to control or fork Awesome to use the characterized
  kernel mechanism.
- **A custom X11 utility:** unnecessary. X remains dormant only for reversible
  stock recovery while Ferrink renders directly to `/dev/fb0`.
- **`openvt` / virtual-terminal switching:** the reviewed device exposes no
  usable `/dev/tty0` or framebuffer console handoff; Xorg owns the framebuffer
  directly.
- **A software checkerboard:** visible in screenshots and tied to Ferrink's
  pixels, but it would not reproduce the stock post-framebuffer treatment and
  would require explicit redraw/restoration of every affected pixel.
- **A one-pixel sharp region for an effectively full-screen effect:** the first
  probe stopped on the expected `EINVAL` before issuing the required repaint,
  so its physical behavior is unknown. Ferrink's intended drawer does not need
  this unproved variant.

## Portability boundary

This evidence applies only to the reviewed KOA3 hardware, kernel, firmware,
framebuffer geometry, and Zelda ABI. PW1 support must remain disabled until its
own display path is characterized. The legacy request number, its unusual
`EINVAL` behavior, rectangle limits, and the required refresh may differ on
other Kindle generations.

Ferrink uses an observed kernel ABI and does not copy Amazon's Lua or native
library code, link to `liblab126IGL`, or distribute a modified Awesome build.
If a future implementation chooses any of those paths, it requires a separate
licensing and distribution review.
