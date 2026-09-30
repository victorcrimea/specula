#!/bin/sh
# Joins a running game as a headless observer and streams it to
# specula-server's feed socket.
#
# usage: scripts/observe.sh HOST PORT [SOCKET]
# PYROGENESIS overrides the engine binary.

set -eu

if [ "$#" -lt 2 ]; then
	echo "usage: $0 HOST PORT [SOCKET]" >&2
	exit 2
fi

host=$1
port=$2
socket=${3:-/tmp/specula-feed.sock}
pyrogenesis=${PYROGENESIS:-$(dirname "$0")/../../0ad-specula/binaries/system/pyrogenesis}

# -unique-logs: a crash or oos_dump must not overwrite the log of a player's
# client running on the same machine.
exec "$pyrogenesis" \
	-autostart-client="$host" \
	-autostart-port="$port" \
	-autostart-nonvisual \
	-autostart-disable-replay \
	-autostart-playername=specula \
	-unique-logs \
	-conf=specula.feed:"$socket"
