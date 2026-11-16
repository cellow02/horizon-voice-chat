#!/bin/sh
# Compiles and tests the mods for the older mod API (legacy/) against a Horizon checkout, the
# way the launcher builds them: as packages of Horizon's own workspace, with Horizon's
# Cargo.lock. A dependency that is not in that lock file fails here, as it would in the launcher.
#
#   tools/check-legacy.sh /path/to/horizon     (a checkout of the commit in horizon-mixin.toml)
#
# The checkout's Cargo.toml and Cargo.lock are changed for the run and put back afterwards.
set -eu
[ $# -eq 1 ] || { echo "usage: $0 <horizon checkout>" >&2; exit 2; }
horizon=$(cd "$1" && pwd)
cd "$(dirname "$0")/.."
tools/sync-protocol.sh --check

stage="$horizon/voice-mod-check"
rm -rf "$stage"
mkdir "$stage"
cp "$horizon/Cargo.toml" "$stage/Cargo.toml.orig"
cp "$horizon/Cargo.lock" "$stage/Cargo.lock.orig"
restore() {
    cp "$stage/Cargo.toml.orig" "$horizon/Cargo.toml"
    cp "$stage/Cargo.lock.orig" "$horizon/Cargo.lock"
    rm -rf "$stage"
}
trap restore EXIT

for mod in client server; do
    cp -r "legacy/$mod/api" "$stage/$mod"
    # The launcher points the game dependency at the source it builds from.
    sed -i 's|^horizon-client = .*|horizon-client = { path = "../../crates/horizon-client" }|' \
        "$stage/$mod/Cargo.toml"
done
sed -i 's|^members = \[|members = [\n    "voice-mod-check/client",\n    "voice-mod-check/server",|' \
    "$horizon/Cargo.toml"

cd "$horizon"
# --offline: nothing may be added to the lock file but the two mod packages themselves.
cargo test --offline -p mod-horizon-voice -p mod-horizon-voice-server
added=$(diff "$stage/Cargo.lock.orig" Cargo.lock | grep '^> name = ' | grep -v '"mod-horizon-voice' || true)
if [ -n "$added" ]; then
    echo "These crates are not in Horizon's Cargo.lock:" >&2
    echo "$added" >&2
    exit 1
fi
echo "Both legacy mods build and pass their tests on $(git -C "$horizon" rev-parse --short HEAD 2>/dev/null || echo "this checkout")."
