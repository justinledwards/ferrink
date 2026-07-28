# KOReader library update

Ferrink's KOReader plugin adds an **Update library** action to KOReader's main
menu. It copies new or changed Markdown books from a private library host and
then closes the connection. Existing Kindle books are never deleted.

The design is intentionally short-lived:

1. KOReader turns Wi-Fi on through its normal network manager.
2. The plugin starts Tailscale in userspace-networking mode. This works on the
   reviewed Oasis 3 even though its kernel has no `/dev/net/tun` device.
3. rclone opens one SFTP transfer through Tailscale's loopback SOCKS5 proxy.
4. `rclone copy` adds or updates only `*.md` files, one transfer at a time.
5. The plugin stops Tailscale on success, failure, or interruption.

There is no always-running scanner or sync daemon. Dismissing the progress
message hides it without abandoning the cleanup-owned shell job.
The whole update also has a 30-minute safety limit; a later run continues the
add/update-only copy if a large first transfer needs more time.

## Private endpoint

The Kindle receives a dedicated SSH key. The library host authorizes that key
with an OpenSSH forced command:

```text
restrict,command="/usr/bin/rclone serve sftp --stdio --read-only /library/root"
```

The key cannot open a shell or write to the source library. Tailscale
authenticates the destination node and encrypts the route before the nested SSH
connection begins. rclone therefore does not add a second `known_hosts` check;
its Go SSH client cannot match that extra pin reliably across the userspace
SOCKS connection and tailnet-only TCP forward used here. The generated key,
Tailscale identity, host address, and collection map live under
`/var/local/ferrink/library-sync`; none belongs in Git.

## Install and enroll

The installer downloads the pinned official ARMv7 archives into the ignored
`target/reference/` directory, verifies fixed SHA-256 values, and stages the
verified binaries before activation. It does not invoke Cargo.

```sh
LIBRARY_SOURCE_SSH=reader@library-host \
LIBRARY_SOURCE_ROOT=/srv/private-library \
LIBRARY_SOURCE_PORT=2222 \
  tools/install-koreader-library-sync install \
  stories=stories/chapters/markdown

tools/install-koreader-library-sync enroll
# Open the one-time auth_url printed by the command.
tools/install-koreader-library-sync finish-enrollment
```

Set `LIBRARY_SOURCE_SSH` and `LIBRARY_SOURCE_ROOT` for the private library
host. `LIBRARY_SOURCE_USER` normally comes from the `user@host` SSH endpoint.
Each `NAME=SOURCE` mapping copies from `SOURCE` below the restricted root into
`/mnt/us/documents/NAME`.

If the library host uses Tailscale SSH, port 22 is intercepted before the
dedicated OpenSSH forced-command key can be evaluated. Keep Tailscale SSH in
place and expose the host's ordinary OpenSSH daemon on a separate tailnet-only
TCP port, then pass that port as `LIBRARY_SOURCE_PORT`. For example, on the
library host:

```sh
sudo tailscale serve --bg --yes --tcp 2222 tcp://127.0.0.1:22
```

After enrollment, `status` reports only whether components and identity state
are present and whether Tailscale/rclone processes remain. It deliberately
does not print the tailnet, host address, node identity, key, or collection
names.

## Verification

`tools/test-koreader-library-sync` uses fake Tailscale and rclone processes to
verify the bounded lifecycle without a Kindle or Rust build. It covers:

- successful multi-collection copy and daemon shutdown;
- preservation of the active updater lock when a second launch is rejected;
- transfer failure, cleanup, and preservation of an existing book; and
- rejection of unsafe private collection paths before rclone runs.

Device acceptance additionally requires one real update, confirmation that the
new files open in KOReader, and a final process check showing zero `tailscaled`
and `rclone` processes.

## Upstream lessons and licenses

The userspace proxy mechanism was confirmed against the MIT-licensed
[`koreader-tailscale`](https://github.com/victoria-riley-barnett/koreader-tailscale)
plugin and Tailscale's official userspace-networking documentation. Ferrink's
plugin is an independent, narrower implementation: it does not use the
upstream unchecked self-installer, global `killall`, persistent manual toggle,
or source code.

The plugin uses KOReader's documented-in-source `NetworkMgr` and `Trapper`
boundaries. KOReader is AGPL-3.0-only. The installer fetches, rather than
vendors, official [Tailscale](https://github.com/tailscale/tailscale) and
[rclone](https://github.com/rclone/rclone) release binaries. Their upstream
licenses and notices remain authoritative for those unmodified binaries.
