#!/bin/sh

# Ferrink already owns the framebuffer, input, screensaver inhibitor, Pillow,
# and exact stock-process lease. This adapter only loads the root-owned Home
# Assistant configuration and replaces itself with the dashboard process.

APP=/mnt/us/slint-kindle-home-assistant
CONFIG=/var/local/slint-home-assistant.env
LOG_DIR=/mnt/us/slint-kindle-home-assistant-logs

if [ ! -x "${APP}" ]; then
    echo "ferrink-home-assistant: application is missing or not executable" >&2
    exit 66
fi
if [ ! -r "${CONFIG}" ]; then
    echo "ferrink-home-assistant: root-owned configuration is unavailable" >&2
    exit 66
fi

set -a
# The existing application contract stores its two credentials and optional
# idle timeout in this root-owned 0600 file.
# shellcheck disable=SC1090
. "${CONFIG}"
set +a

umask 077
mkdir -p "${LOG_DIR}" || exit 73
RUN_TS=$(/bin/date +%Y%m%dT%H%M%S) || exit 74
export RUN_TS

exec "${APP}" >>"${LOG_DIR}/${RUN_TS}.log" 2>&1
