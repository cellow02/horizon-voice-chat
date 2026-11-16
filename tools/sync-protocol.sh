#!/bin/sh
# Copies the shared sources into every mod. A Horizon native mod may not reference files outside
# its own folder, so each mod carries copies. Run this after editing shared/, or the client's
# net.rs, mixer.rs and audio.rs, or the server's udp.rs and config.rs.
#
#   shared/protocol/        -> the protocol of all four mods
#   shared/legacy/gcm.rs    -> replaces protocol/gcm.rs in the mods for the older mod API
#   client/src, server/src  -> the game-independent files of the mods for the older mod API
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

for mod in client/src server/src legacy/client/api/src legacy/server/api/src; do
    if [ "$check" != "--check" ]; then
        rm -rf "$mod/protocol"
        mkdir "$mod/protocol"
    fi
    for file in crypto.rs mod.rs packets.rs wire.rs; do
        sync "shared/protocol/$file" "$mod/protocol/$file"
    done
    case "$mod" in
        legacy/*) sync shared/legacy/gcm.rs "$mod/protocol/gcm.rs" ;;
        *) sync shared/protocol/gcm.rs "$mod/protocol/gcm.rs" ;;
    esac
done
for file in net.rs mixer.rs audio.rs; do
    sync "client/src/$file" "legacy/client/api/src/$file"
done
for file in udp.rs config.rs; do
    sync "server/src/$file" "legacy/server/api/src/$file"
done
exit $status
