# Architecture

Ferrink has three small layers:

```text
your apps  <->  Ferrink launcher and supervisor  <->  reader-specific adapter
```

The launcher is a Slint application: it draws the home screen and asks for an
action such as “open this app” or “return to the reader's own screen.” The
supervisor is responsible for starting an app, waiting for it to finish, and
bringing the launcher back. The device adapter does the carefully limited work
of checking a local profile, drawing to a framebuffer, refreshing the E Ink
panel, and turning touches into ordinary Slint pointer events.

This separation is useful for tinkerers: the UI can be built and tested on a
computer, while reader-specific work stays in one well-marked place. A new
device needs a local profile and evidence that its display and touch details
match. The supplied `reference-*` files exist only to exercise that code in
tests.

The project keeps the vendor interface available as a recovery and compatibility
destination. Ferrink does not bundle vendor software or claim that every
Linux-based reader has the same display, input, power, or boot behavior.
