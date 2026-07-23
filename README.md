# ferrink

Ferrink is a small, experimental Rust launcher for Linux-based E Ink readers.
It gives locally installed apps a calm home screen, hands off cleanly to those
apps, and comes back when they close. The UI uses Slint; the rest is ordinary
Rust with a deliberately small dependency set.

Ferrink is unofficial software and is not affiliated with or endorsed by
Amazon, Slint, KOReader, or any device manufacturer.

## A note before you tinker

This is source code for curious tinkerers, not a one-click device installer.
It contains host tests, example profiles, and reversible tools. It deliberately
does **not** include anyone's device details, screenshots, network settings,
keys, passwords, or app configuration.

The checked-in `reference-*` profiles are made-up test data; they are not a
recipe for a particular reader. If you try Ferrink on your own hardware, make
your own local profile, keep a way back to stock software, and take it one
small step at a time.

## Build it

```sh
cargo fmt --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo zigbuild -p ferrink-shell --bin ferrink-shell-kindle \
  --features kindle-runtime --target armv7-unknown-linux-musleabihf --release
```

The release build is tuned for a compact binary. `cargo zigbuild` is used for
cross-compiled device builds so the necessary C headers and target support are
available without a bespoke toolchain.

## If you connect a reader

Keep real connection details out of this folder. `tools/kindle` reads the path
you supply in `KINDLE_SSH_CONFIG`; start with the harmless
[`tools/kindle-ssh.config.example`](tools/kindle-ssh.config.example) and save
your real copy elsewhere.

The boot installer also asks you for a local `FERRINK_DEVICE_PROFILE` and
`FERRINK_STOCK_GUI_SHA256`. They are intentionally not in the repository. The
[device-tool guide](docs/KINDLE_DEVICE_TOOL.md) explains what each tool does
before it changes anything.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Device-tool guide](docs/KINDLE_DEVICE_TOOL.md)
- [Upstream research boundaries](docs/UPSTREAM_RESEARCH.md)
- [Licensing and distribution](docs/LICENSING_AND_DISTRIBUTION.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
- [Acknowledgments](ACKNOWLEDGMENTS.md)
- [Public-source audit](PUBLIC_SOURCE_AUDIT.md)
- [Contributing](CONTRIBUTING.md)

## License

Ferrink project source is licensed under [GPL-3.0-only](LICENSE). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for dependencies, fonts, and
their required notices.
