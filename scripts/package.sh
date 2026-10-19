#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: scripts/package.sh <target> <path to the quill binary> <output folder>" >&2
    exit 2
fi

target="$1"
binary="$2"
out="$3"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

mkdir -p "$out"
cp "$binary" LICENSE README.md "$stage/"

case "$target" in
    *windows*)
        name="quill-$target.zip"
        rm -f "$out/$name"
        powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$stage")\\*' -DestinationPath '$(cygpath -w "$out")\\$name' -Force"
        ;;
    *)
        name="quill-$target.tar.gz"
        chmod 755 "$stage/quill"
        tar -czf "$out/$name" -C "$stage" quill LICENSE README.md
        ;;
esac

cd "$out"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$name" | sed 's/ [*]/  /' > "$name.sha256"
else
    shasum -a 256 "$name" > "$name.sha256"
fi
echo "packed $out/$name"