#!/bin/sh
# Writes the registry dependency lock (Cargo.lock) of a mod, the file the Horizon launcher
# requires beside Cargo.toml. It does what `horizon-launcher native-lock <mod>` does: resolve the
# mod's crates.io dependencies without the SDK. Use the launcher's command when you have it.
#
#   tools/make-lock.sh client [path/to/horizon/checkout]
#
# With a Horizon checkout, crates the game already uses keep the game's versions, so the mod
# and the game share them.
set -eu
mod="$(cd "$1" && pwd)"
game="${2:-}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# The manifest without the SDK dependency and with an empty library.
grep -v '^horizon-mod-api' "$mod/Cargo.toml" > "$work/Cargo.toml"
printf '\n[lib]\npath = "inert.rs"\n\n[workspace]\n' >> "$work/Cargo.toml"
: > "$work/inert.rs"
if [ -n "$game" ]; then
    cp "$game/Cargo.lock" "$work/Cargo.lock"
    # Keeps the pins the mod needs, drops the rest, adds what the game does not have.
    cargo metadata --manifest-path "$work/Cargo.toml" --format-version 1 > /dev/null
else
    cargo generate-lockfile --manifest-path "$work/Cargo.toml"
fi
cp "$work/Cargo.lock" "$mod/Cargo.lock"
echo "wrote $mod/Cargo.lock"
