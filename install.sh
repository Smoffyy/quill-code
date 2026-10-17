#!/bin/sh
set -eu

repo="Smoffyy/quill-code"
version="${QUILL_VERSION:-latest}"
dir="${QUILL_INSTALL_DIR:-$HOME/.local/bin}"

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'quill install: %s\n' "$*" >&2
    exit 1
}

case "$(uname -s)" in
    Linux) system="unknown-linux-musl" ;;
    Darwin) system="apple-darwin" ;;
    *) fail "there is no quill build for $(uname -s). On Windows, run in PowerShell: irm https://github.com/$repo/releases/latest/download/install.ps1 | iex" ;;
esac

machine="$(uname -m)"
if [ "$system" = "apple-darwin" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
    machine="arm64"
fi
case "$machine" in
    x86_64 | amd64) arch="x86_64" ;;
    arm64 | aarch64) arch="aarch64" ;;
    *) fail "there is no quill build for $machine processors yet." ;;
esac

target="$arch-$system"
archive="quill-$target.tar.gz"
if [ -n "${QUILL_DOWNLOAD_URL:-}" ]; then
    base="$QUILL_DOWNLOAD_URL"
elif [ "$version" = "latest" ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    base="https://github.com/$repo/releases/download/v${version#v}"
fi

fetch() {
    case "$1" in
        http://* | https://*)
            if command -v curl >/dev/null 2>&1; then
                curl -fsSL --retry 3 -o "$2" "$1"
            elif command -v wget >/dev/null 2>&1; then
                wget -q -O "$2" "$1"
            else
                fail "curl or wget is needed to download quill."
            fi
            ;;
        *) cp "$1" "$2" ;;
    esac
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

say "Downloading quill for $target..."
fetch "$base/$archive" "$work/$archive" || fail "could not download $base/$archive"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || fail "could not download the checksums from $base"

expected="$(grep " $archive\$" "$work/SHA256SUMS" | cut -d ' ' -f 1)"
[ -n "$expected" ] || fail "the release has no checksum for $archive."
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$work/$archive" | cut -d ' ' -f 1)"
else
    actual="$(shasum -a 256 "$work/$archive" | cut -d ' ' -f 1)"
fi
[ "$expected" = "$actual" ] || fail "the download is damaged (checksum mismatch). Try again."

tar -xzf "$work/$archive" -C "$work"
mkdir -p "$dir"
cp "$work/quill" "$dir/quill.new"
chmod 755 "$dir/quill.new"
mv -f "$dir/quill.new" "$dir/quill"

installed="$("$dir/quill" --version)" || fail "quill was copied to $dir but does not start."
say "Installed $installed to $dir/quill"

case ":$PATH:" in
    *":$dir:"*)
        say "Run quill in any project folder to start."
        exit 0
        ;;
esac

if [ -n "${QUILL_NO_MODIFY_PATH:-}" ]; then
    say "Add $dir to your PATH to run quill from anywhere."
    exit 0
fi

line="export PATH=\"$dir:\$PATH\""
case "$(basename "${SHELL:-sh}")" in
    zsh) profiles="$HOME/.zshrc" ;;
    bash)
        if [ "$system" = "apple-darwin" ]; then
            profiles="$HOME/.bash_profile"
        else
            profiles="$HOME/.bashrc"
        fi
        ;;
    fish)
        profiles=""
        mkdir -p "$HOME/.config/fish/conf.d"
        printf 'fish_add_path %s\n' "$dir" > "$HOME/.config/fish/conf.d/quill.fish"
        ;;
    *) profiles="$HOME/.profile" ;;
esac
for profile in $profiles; do
    if ! grep -qsF "$line" "$profile"; then
        printf '\n%s\n' "$line" >> "$profile"
        say "Added $dir to your PATH in $profile."
    fi
done

say "Open a new terminal, then run quill in any project folder."