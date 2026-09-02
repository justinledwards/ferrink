#!/bin/sh

# Ferrink already owns the framebuffer, input, screensaver inhibitor, Pillow,
# and exact stock-process lease. This adapter only validates the installed
# application and replaces itself with the game process.

APP=/mnt/us/ferrink-wordpuzzle

if [ ! -f "${APP}" ] || [ ! -x "${APP}" ] || [ -L "${APP}" ]; then
    echo "ferrink-wordpuzzle: application is missing, unsafe, or not executable" >&2
    exit 66
fi

exec "${APP}"
