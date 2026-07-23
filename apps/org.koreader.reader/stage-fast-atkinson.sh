#!/bin/sh
set -eu

expected_revision=aeae0775d9251365eae3b133cbf26ce0366f6108
source_dir=${1:-}
destination=${2:-}

if [ -z "$source_dir" ] || [ -z "$destination" ]; then
    echo "usage: stage-fast-atkinson.sh FAST_FONT_CHECKOUT DESTINATION" >&2
    exit 2
fi

asset_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/font-assets" && pwd)
font_source="$source_dir/fast-fonts-for-kindle/Fast_Atkinson"

if [ -e "$destination" ] && [ ! -d "$destination" ]; then
    echo "destination exists and is not a directory: $destination" >&2
    exit 1
fi
if [ -d "$destination" ] && [ -n "$(find "$destination" -mindepth 1 -print -quit)" ]; then
    echo "destination is not empty: $destination" >&2
    exit 1
fi

if [ -d "$source_dir/.git" ]; then
    actual_revision=$(git -C "$source_dir" rev-parse HEAD)
    if [ "$actual_revision" != "$expected_revision" ]; then
        echo "unexpected Fast-Font revision: $actual_revision" >&2
        exit 1
    fi
fi

file_hash() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    else
        sha256sum "$1" | awk '{ print $1 }'
    fi
}

verify_face() {
    filename=$1
    expected_hash=$2
    path="$font_source/$filename"
    if [ ! -f "$path" ]; then
        echo "missing Fast Atkinson face: $path" >&2
        exit 1
    fi
    actual_hash=$(file_hash "$path")
    if [ "$actual_hash" != "$expected_hash" ]; then
        echo "hash mismatch for $filename" >&2
        exit 1
    fi
}

verify_face Fast_Atkinson_Regular.otf 15b04dc0f088df1a0bb28b1a332ceed8c12079fba842708da6c296da77001098
verify_face Fast_Atkinson_Bold.otf 746fbd33626c10517b26e8c6051a6a467362f934021487cd20b0eb57e0487180
verify_face Fast_Atkinson_Italic.otf e9a018739a60dccb2dcaa4c2b2bf57eeb4beea17fed543bc5c4ff0e1532951b5
verify_face Fast_Atkinson_BoldItalic.otf 74ff791e06152fbd8b1969e90da54799fec6d29f06ba10ee5e20f1bc49a3f096

mkdir -p "$destination"
for filename in \
    Fast_Atkinson_Regular.otf \
    Fast_Atkinson_Bold.otf \
    Fast_Atkinson_Italic.otf \
    Fast_Atkinson_BoldItalic.otf
do
    cp "$font_source/$filename" "$destination/$filename"
done
cp "$asset_dir/SHA256SUMS" "$destination/SHA256SUMS"
cp "$asset_dir/Atkinson-Hyperlegible-OFL-1.1.txt" "$destination/OFL-1.1.txt"
cp "$asset_dir/Fast-Font-MIT.txt" "$destination/Fast-Font-MIT.txt"

echo "staged Fast Atkinson from $expected_revision in $destination"
