#!/bin/sh

# Ferrink already owns the Kindle foreground lease. This adapter intentionally
# excludes KOReader's stock-launcher transitions (Pillow, awesome, services,
# firewall, passcode, and framebuffer restoration) while preserving the
# environment and restart contract needed by the packaged reader.

KOREADER_DIR=/mnt/us/koreader
ACTIVE_STARTUP_SCRIPT=/var/tmp/koreader.sh
RESTART_EXIT_CODE=85

if [ ! -d "${KOREADER_DIR}" ] || [ ! -x "${KOREADER_DIR}/reader.lua" ]; then
    echo "ferrink-koreader: incomplete KOReader installation" >&2
    exit 66
fi

cd "${KOREADER_DIR}" || exit 66

# KOReader deliberately compares this active copy with the installed launcher
# to detect self-updates. Keep that contract even though Ferrink does not run
# the stock-facing launcher itself.
if ! cp -pf "${KOREADER_DIR}/koreader.sh" "${ACTIVE_STARTUP_SCRIPT}"; then
    echo "ferrink-koreader: cannot stage startup-script update guard" >&2
    exit 74
fi
chmod 700 "${ACTIVE_STARTUP_SCRIPT}" || exit 74

export KOREADER_DIR
export LC_ALL=en_US.UTF-8
export STARDICT_DATA_DIR=data/dict
export EXT_FONT_DIR='/usr/java/lib/fonts;/mnt/us/fonts;/var/local/font/mnt;/mnt/us/linkfonts/fonts'
export STOP_FRAMEWORK=no
export AWESOME_STOPPED=no
export CVM_STOPPED=no
export VOLUMD_STOPPED=no

reader_pid=
# Invoked by the signal-trap callback below.
# shellcheck disable=SC2329
terminate_reader() {
    if [ -n "${reader_pid}" ] && kill -0 "${reader_pid}" 2>/dev/null; then
        kill -TERM "${reader_pid}" 2>/dev/null
        wait "${reader_pid}" 2>/dev/null
    fi
}
# Registered by name with trap below.
# shellcheck disable=SC2329
finish_interrupted() {
    terminate_reader
    rm -f "${ACTIVE_STARTUP_SCRIPT}"
    exit 143
}
trap finish_interrupted HUP INT TERM

return_value=${RESTART_EXIT_CODE}
while [ "${return_value}" -eq "${RESTART_EXIT_CODE}" ]; do
    ./reader.lua "$@" >>crash.log 2>&1 &
    reader_pid=$!
    wait "${reader_pid}"
    return_value=$?
    reader_pid=
done

rm -f "${ACTIVE_STARTUP_SCRIPT}"
exit "${return_value}"
