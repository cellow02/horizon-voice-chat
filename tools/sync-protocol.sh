#!/bin/sh
# Copies the shared protocol into both mods. A Horizon native mod may not reference files outside
# its own folder, so each mod carries a copy. Run this after editing shared/protocol/.
#
# With --check it changes nothing and fails when a copy differs.
set -eu
cd "$(dirname "$0")/.."
check=${1:-}
status=0

# sync SOURCE TARGET: a file or a folder.
sync() {
    if [ "$check" = "--check" ]; then
        diff -r "$1" "$2" >/dev/null 2>&1 || {
            echo "$2 differs from $1: run tools/sync-protocol.sh" >&2
            status=1
        }
    else
        rm -rf "$2"
        cp -r "$1" "$2"
    fi
}

for mod in client/src server/src; do
    if [ "$check" != "--check" ]; then
        rm -rf "$mod/protocol"
        mkdir "$mod/protocol"
    fi
    for file in crypto.rs mod.rs packets.rs wire.rs; do
        sync "shared/protocol/$file" "$mod/protocol/$file"
    done
    sync shared/protocol/gcm.rs "$mod/protocol/gcm.rs"
done
exit $status
