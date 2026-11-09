#!/bin/sh
# Hosts a game for Specula's web players, headless, and streams it to
# specula-server's feed socket, which writes the players' commands back.
# The host only observes; the web players' slots stay unassigned.
#
# usage: scripts/play.sh SOCKET PORT [-autostart-... options]
# PYROGENESIS overrides the engine binary.

set -eu

if [ "$#" -lt 2 ]; then
	echo "usage: $0 SOCKET PORT [options]" >&2
	exit 2
fi

socket=$1
port=$2
shift 2
pyrogenesis=${PYROGENESIS:-$(dirname "$0")/../../0ad-specula/binaries/system/pyrogenesis}

# -unique-logs: a crash or oos_dump must not overwrite the log of another
# engine running on the same machine.
exec "$pyrogenesis" \
	-autostart-host \
	-autostart-specula-play \
	-autostart-port="$port" \
	-autostart-nonvisual \
	-autostart-disable-replay \
	-autostart-playername=specula \
	-unique-logs \
	-conf=specula.feed:"$socket" \
	"$@"
