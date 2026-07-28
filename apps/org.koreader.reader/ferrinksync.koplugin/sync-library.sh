#!/bin/sh

# Add/update-only KOReader library sync. The installer supplies the private
# endpoint, host key, reader key, and collection map outside the userstore.

set -u

config_dir=${FERRINK_SYNC_CONFIG_DIR:-/var/local/ferrink/library-sync}
config_file=$config_dir/rclone.conf
collections_file=$config_dir/collections
state_dir=$config_dir/tailscale-state
tools_dir=${FERRINK_SYNC_TOOLS_DIR:-/mnt/us/ferrink/tools}
documents_dir=${FERRINK_SYNC_DOCUMENTS_DIR:-/mnt/us/documents}
temporary_dir=${FERRINK_SYNC_TEMPORARY_DIR:-/tmp}
timeout_tool=${FERRINK_SYNC_TIMEOUT_TOOL:-/bin/busybox}
sync_budget_seconds=${FERRINK_SYNC_BUDGET_SECONDS:-1800}
rclone=$tools_dir/rclone-v1.74.4-armv7
tailscale=$tools_dir/tailscale-v1.98.9-armv7
tailscaled=$tools_dir/tailscaled-v1.98.9-armv7
lock_dir=$temporary_dir/ferrink-library-sync.lock
socket=$temporary_dir/ferrink-tailscaled.$$.sock
run_log=$temporary_dir/ferrink-library-sync.$$.log
result_file=$temporary_dir/ferrink-library-sync.$$.result
daemon_pid=
started_daemon=0
lock_acquired=0
updated=0

write_result() {
    printf '%s\n' "$1" >"$result_file"
}

fail() {
    write_result "$1"
    exit 1
}

# Registered by name with trap below.
# shellcheck disable=SC2329
finish() {
    status=$?
    trap - 0 HUP INT TERM

    if [ "$started_daemon" -eq 1 ]; then
        "$tailscale" --socket="$socket" down >>"$run_log" 2>&1 || true
        if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
            kill -TERM "$daemon_pid" 2>/dev/null || true
            wait "$daemon_pid" 2>/dev/null || true
        fi
    fi

    if [ "$lock_acquired" -eq 1 ]; then
        rmdir "$lock_dir" 2>/dev/null || true
    fi

    if [ ! -s "$result_file" ]; then
        if [ "$status" -eq 0 ]; then
            write_result "Library update complete. Updated $updated collections."
        else
            write_result "The library update failed safely. No books were deleted."
        fi
    fi

    cat "$result_file"
    rm -f "$run_log" "$result_file" "$socket"
    exit "$status"
}

trap finish 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

umask 077
: >"$run_log" || fail "The library update could not create its temporary log."
: >"$result_file" || fail "The library update could not create its result file."

if ! mkdir "$lock_dir" 2>/dev/null; then
    fail "A library update is already running."
fi
lock_acquired=1

[ -x "$rclone" ] || fail "Library updater setup is incomplete (rclone is missing)."
[ -x "$tailscale" ] || fail "Library updater setup is incomplete (Tailscale is missing)."
[ -x "$tailscaled" ] || fail "Library updater setup is incomplete (tailscaled is missing)."
[ -x "$timeout_tool" ] || fail "Library updater setup is incomplete (timeout helper is missing)."
[ -s "$config_file" ] || fail "Library updater setup is incomplete (connection settings are missing)."
[ -s "$collections_file" ] || fail "Library updater setup is incomplete (no collections are selected)."
case $sync_budget_seconds in
    ''|*[!0-9]*) fail "The library update time limit is invalid." ;;
esac
if [ "$sync_budget_seconds" -lt 60 ] || [ "$sync_budget_seconds" -gt 7200 ]; then
    fail "The library update time limit is invalid."
fi
if [ $((sync_budget_seconds % 60)) -eq 0 ]; then
    sync_budget_minutes=$((sync_budget_seconds / 60))
    if [ "$sync_budget_minutes" -eq 1 ]; then
        sync_limit_label="1-minute"
    else
        sync_limit_label="$sync_budget_minutes-minute"
    fi
else
    sync_limit_label="$sync_budget_seconds-second"
fi
sync_limit_message="The library update reached its $sync_limit_label safety limit. Try again to continue."
sync_deadline=$(( $(date +%s) + sync_budget_seconds ))

if ! mkdir -p "$state_dir" >>"$run_log" 2>&1; then
    fail "The library updater could not open its private Tailscale state."
fi
chmod 700 "$config_dir" "$state_dir" >>"$run_log" 2>&1 || \
    fail "The library updater could not protect its private settings."

if ! ifconfig lo 2>/dev/null | grep -q '127\.0\.0\.1'; then
    fail "The Kindle loopback interface is unavailable."
fi

SSL_CERT_FILE=/mnt/us/koreader/data/ca-bundle.crt \
    "$tailscaled" \
    --statedir="$state_dir" \
    --socket="$socket" \
    --tun=userspace-networking \
    --socks5-server=127.0.0.1:1055 \
    --outbound-http-proxy-listen=127.0.0.1:1056 \
    >>"$run_log" 2>&1 &
daemon_pid=$!
started_daemon=1

ready=0
attempt=0
while [ "$attempt" -lt 10 ]; do
    if [ -S "$socket" ]; then
        ready=1
        break
    fi
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
        break
    fi
    sleep 1
    attempt=$((attempt + 1))
done
[ "$ready" -eq 1 ] || fail "The private Tailscale connection could not start."

if ! SSL_CERT_FILE=/mnt/us/koreader/data/ca-bundle.crt \
    "$tailscale" --socket="$socket" up \
    --timeout=30s \
    --accept-dns=false \
    --accept-routes=false \
    --hostname=ferrink-reader \
    --netfilter-mode=off \
    --shields-up=true \
    >>"$run_log" 2>&1; then
    fail "The Kindle is not signed in to its private Tailscale connection."
fi

while IFS='|' read -r destination source || [ -n "${destination:-}${source:-}" ]; do
    case ${destination:-} in
        ''|'#'*)
            continue
            ;;
        *[!A-Za-z0-9._-]*)
            fail "A library destination in the private settings is invalid."
            ;;
    esac
    case ${source:-} in
        ''|/*|*..*|*[!A-Za-z0-9._/-]*)
            fail "A library source in the private settings is invalid."
            ;;
    esac

    destination_dir=$documents_dir/$destination
    if ! mkdir -p "$destination_dir" >>"$run_log" 2>&1; then
        fail "The destination for $destination could not be created."
    fi

    remaining_seconds=$(( sync_deadline - $(date +%s) ))
    if [ "$remaining_seconds" -le 0 ]; then
        fail "$sync_limit_message"
    fi
    if ! "$timeout_tool" timeout -t "$remaining_seconds" -s TERM "$rclone" copy \
        --config "$config_file" \
        --transfers 1 \
        --checkers 1 \
        --buffer-size 0 \
        --include '*.md' \
        --exclude '*' \
        "library:$source" \
        "$destination_dir" \
        >>"$run_log" 2>&1; then
        if [ "$(date +%s)" -ge "$sync_deadline" ]; then
            fail "$sync_limit_message"
        fi
        fail "The $destination collection could not be updated. No books were deleted."
    fi
    updated=$((updated + 1))
done <"$collections_file"

[ "$updated" -gt 0 ] || fail "No valid library collections were configured."
write_result "Library update complete. Updated $updated collections."
exit 0
