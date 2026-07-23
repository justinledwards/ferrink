# Contributing

Small, reviewable changes are welcome. Please keep a change easy to understand
and run the checks before opening a pull request:

```sh
cargo fmt --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
tools/audit-public-source
```

If you work with a physical reader, keep its profiles, reports, screenshots,
network addresses, keys, and application settings outside the checkout. Share
only synthetic fixtures or material you have deliberately cleaned for public
use. Do not turn an experiment into a supported-device claim without a
reproducible profile, recovery notes, and maintainer review.

Please add an attribution and license note when code, a font, an icon, or an
image comes from another project.
