#!/bin/sh
# Copies shared/protocol/ into both mods. A Horizon native mod may not reference files
# outside its own folder, so each mod carries a copy. Run this after editing shared/protocol/.
# With --check it changes nothing and fails when a copy differs.
set -eu
cd "$(dirname "$0")/.."
status=0
for mod in client server; do
    if [ "${1:-}" = "--check" ]; then
        diff -r shared/protocol "$mod/src/protocol" >/dev/null || {
            echo "$mod/src/protocol differs from shared/protocol: run tools/sync-protocol.sh" >&2
            status=1
        }
    else
        rm -rf "$mod/src/protocol"
        cp -r shared/protocol "$mod/src/protocol"
    fi
done
exit $status
