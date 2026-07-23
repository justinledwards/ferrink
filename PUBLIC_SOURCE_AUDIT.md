# Public-source audit

This repository was created as a fresh public Git history. It contains a
reviewed selection of Ferrink source, tests, packaging, project-authored design
assets, and documentation. It does not mirror the private development history.

## What stays private

The public tree excludes real device profiles, raw probe reports, screenshots,
connection settings, host keys, credentials, application data, and any
user-supplied or uncertain-provenance artwork. The `reference-*` profiles and
reports in this checkout are fully synthetic test fixtures.

## How to check a checkout

Run:

```sh
tools/audit-public-source
```

The script checks for known private-path categories and legacy identifiers.
It complements, but cannot replace, a human review of every new file. CI runs
the same check and a secret scan.

## Before publishing a change

1. Read every new text file, image, and fixture as if it were someone else's
   data.
2. Remove or replace device-specific facts with synthetic examples.
3. Verify licenses and add notices for copied code, fonts, images, or data.
4. Run the contributor checks in [CONTRIBUTING.md](CONTRIBUTING.md).
