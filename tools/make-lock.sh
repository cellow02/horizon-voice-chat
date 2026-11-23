#!/bin/sh
# Writes the registry dependency lock (Cargo.lock) of a mod against the game it is built for.
#
#   tools/make-lock.sh client path/to/game/Cargo.lock
#
# The game's Cargo.lock is the one of the Horizon revision in the mod's horizon-mod.toml
# (`[native] horizon`), from a checkout of horizon-rust/horizon at that commit. A client build
# compiles only the mods against the prebuilt game, so every crate the mod shares with the game
# must keep the game's version and dependency edges. `horizon-launcher native-lock` resolves the
# newest versions instead, which the launcher then refuses.
#
# Steps: resolve the mod's dependencies seeded with the game's lock, give every package the game
# also locks the game's record (tools/lock.py align), let Cargo drop what the mod does not reach
# (as the launcher does with `cargo update --workspace`), and check the result as the launcher
# would (tools/lock.py check). Needs cargo and python3.
set -eu
tools="$(cd "$(dirname "$0")" && pwd)"
mod="$(cd "$1" && pwd)"
game="$2"
[ -d "$game" ] && game="$game/Cargo.lock"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# The manifest without the SDK dependency and with an empty library, like the launcher stages it.
grep -v '^horizon-mod-api' "$mod/Cargo.toml" > "$work/Cargo.toml"
printf '\n[lib]\npath = "inert.rs"\n\n[workspace]\n' >> "$work/Cargo.toml"
: > "$work/inert.rs"
cp "$game" "$work/Cargo.lock"
cargo metadata --manifest-path "$work/Cargo.toml" --format-version 1 > /dev/null
python3 "$tools/lock.py" align "$work/Cargo.lock" "$game"
cp "$work/Cargo.lock" "$work/pins.lock"
cargo update --workspace --manifest-path "$work/Cargo.toml"
python3 "$tools/lock.py" check "$work/Cargo.lock" "$game" "$work/pins.lock"
cp "$work/Cargo.lock" "$mod/Cargo.lock"
echo "wrote $mod/Cargo.lock"
