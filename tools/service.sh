#!/usr/bin/env bash
# Runs the game server as a systemd --user service, which lingering starts at boot (Linux only).
# Usage: tools/service.sh start --exe <cake-server> --listen <addr:port> --client-dir <dir>
#        tools/service.sh stop
# start (re)starts the service on the given build and settings and returns once it answers HTTP;
# stop stops it and keeps it from starting at boot. Both are safe to rerun.
set -euo pipefail
shopt -s nullglob

UNIT_NAME=cake-server.service
UNIT_PATH=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$UNIT_NAME
READY_TIMEOUT_S=60

usage() {
    echo "usage: tools/service.sh start --exe <cake-server> --listen <addr:port> --client-dir <dir> | stop" >&2
    exit 2
}

format_unit() {
    local exe=$1 listen=$2 client_dir=$3
    cat <<EOF
[Unit]
Description=Cake Cut game server
After=network-online.target

[Service]
ExecStart="$exe" --listen "$listen" --client-dir "$client_dir"
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF
}

ensure_linger() {
    if [[ $(loginctl show-user "$USER" --property=Linger --value) == yes ]]; then
        return
    fi
    loginctl enable-linger "$USER"
}

ensure_unit() {
    local unit
    unit=$(format_unit "$@")
    if printf '%s\n' "$unit" | cmp -s - "$UNIT_PATH"; then
        return
    fi
    mkdir -p "$(dirname "$UNIT_PATH")"
    printf '%s\n' "$unit" > "$UNIT_PATH"
    systemctl --user daemon-reload
}

wait_ready() {
    local listen=$1
    for (( waited_s = 0; waited_s < READY_TIMEOUT_S; waited_s++ )); do
        if curl -fs -o /dev/null "http://$listen/"; then
            return
        fi
        sleep 1
    done
    echo "Error: $UNIT_NAME did not answer on http://$listen/ within $READY_TIMEOUT_S s" >&2
    journalctl --user -u "$UNIT_NAME" -n 20 --no-pager >&2 || true
    exit 1
}

start() {
    local exe='' listen='' client_dir=''
    while [[ $# -gt 0 ]]; do
        case $1 in
            --exe) exe=${2:?--exe needs a path}; shift 2 ;;
            --listen) listen=${2:?--listen needs an address}; shift 2 ;;
            --client-dir) client_dir=${2:?--client-dir needs a path}; shift 2 ;;
            *) usage ;;
        esac
    done
    [[ -n $exe && -n $listen && -n $client_dir ]] || usage
    [[ -x $exe ]] || { echo "Error: $exe is not an executable" >&2; exit 1; }
    [[ -d $client_dir ]] || { echo "Error: $client_dir is not a directory" >&2; exit 1; }

    ensure_linger
    ensure_unit "$exe" "$listen" "$client_dir"
    systemctl --user enable -q "$UNIT_NAME"
    systemctl --user restart "$UNIT_NAME"
    wait_ready "$listen"
    echo "$UNIT_NAME: running on http://$listen/ (journalctl --user -u $UNIT_NAME for its log)"
}

stop() {
    [[ $# -eq 0 ]] || usage
    if [[ ! -f $UNIT_PATH ]]; then
        echo "$UNIT_NAME: not installed"
        return
    fi
    systemctl --user disable -q --now "$UNIT_NAME"
    echo "$UNIT_NAME: stopped"
}

[[ $# -ge 1 ]] || usage
[[ $(uname -s) == Linux ]] || { echo "Error: the server runs as a service only on Linux; elsewhere make serve runs it in the foreground" >&2; exit 1; }
# cron and other sessions without a login leave this unset, and systemctl --user needs it.
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
command=$1
shift
case $command in
    start) start "$@" ;;
    stop) stop "$@" ;;
    *) usage ;;
esac
