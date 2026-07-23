# Device-tool guide

The helpers in `tools/` are development tools, not a promise that an arbitrary
device is safe to operate. They deliberately require local configuration and
refuse to inherit a checked-in connection.

## Local connection configuration

Copy `tools/kindle-ssh.config.example` **outside** the checkout, replace the
placeholder host and identity path, then set the environment explicitly:

```sh
export KINDLE_SSH_CONFIG="$HOME/.config/ferrink/kindle-ssh.config"
export KINDLE_TARGET=kindle
tools/kindle ping
```

Never commit a device address, hostname, host key, identity-file path, account
configuration, screenshot, or raw report. `tools/kindle` refuses every remote
operation unless `KINDLE_SSH_CONFIG` names an existing local regular file.

## Deliberate operations

```sh
tools/kindle state
tools/kindle run uname -a
tools/kindle script value <<'REMOTE'
printf 'argument=%s\n' "$1"
REMOTE
tools/kindle shot review
tools/kindle gc16
tools/kindle push local-file /mnt/us/ferrink/staging/file
tools/kindle pull /mnt/us/ferrink/report.json target/device-reports/report.json
```

`run` quotes each argument before sending it to the remote shell. `script`
accepts a quoted heredoc and passes any following arguments literally. Prefer
those forms over ad-hoc remote shell quoting. `shot` writes only to ignored
`target/device-evidence/`; review it locally and do not promote it into source
control.

## Build and deployment

Run formatter, tests, and strict Clippy before a release build. Build ARM
artifacts with `cargo zigbuild`; `tools/deploy-kindle-shell` does not build an
artifact, change boot configuration, delete old runs, or retry a failed launch.
It stages one artifact set through a fresh userstore path and verifies its hash.

The public helper installs only the project-authored Topography background.
Other images remain local files under `/mnt/us/ferrink/backgrounds` and must be
licensed by their owner. The picker accepts only non-symlinked, bounded,
632×843 RGB8 PNG files from that one directory.

## Boot preview

`tools/install-kindle-boot` is deliberately incomplete without two locally
reviewed inputs:

```sh
export FERRINK_DEVICE_PROFILE=/secure/local/device-profile.toml
export FERRINK_STOCK_GUI_SHA256=lowercase_sha256_of_reviewed_stock_job
tools/install-kindle-boot install
```

The committed `device-profiles/reference-*.toml` files are synthetic unit-test
fixtures, not installable profiles. Keep a real profile outside the repository,
use only after reviewing the recovery flow, and retain an independent stock
recovery path. The installer refuses to continue without both local inputs.
