# ferrink

Ferrink is a Rust launcher and home screen for E Ink readers. It opens locally
installed apps, gives them the screen while they run, and returns to Ferrink
when they close.

It is built with Slint and designed to feel at home on a compact, grayscale,
touch-first reader.

## Status

Ferrink is an early project for people who enjoy adapting software to their own
hardware. It is not a ready-made installer yet. The sample `reference-*`
profiles are test fixtures, not instructions for a particular reader.

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

## Using it with a reader

The project includes tools for local development and a reversible boot setup.
The [device-tool guide](docs/KINDLE_DEVICE_TOOL.md) explains what each tool
does, what it needs, and how to keep your own configuration local.

## More detail

- [Architecture](docs/ARCHITECTURE.md)
- [Device-tool guide](docs/KINDLE_DEVICE_TOOL.md)
- [Acknowledgments](ACKNOWLEDGMENTS.md)
- [Contributing](CONTRIBUTING.md)

## License

Ferrink project source is licensed under [GPL-3.0-only](LICENSE). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for dependencies, fonts, and
their required notices.
