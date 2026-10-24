#!/bin/bash
# Entrypoint of the rdlab-client image.
#
# Environment:
#   RD_ROLE          controlled | controller | shell
#   RD_SERVER        rendezvous server host (custom-rendezvous-server), e.g. 198.18.10.10
#   RD_RELAY         relay server (relay-server), e.g. 198.18.10.11
#   RD_KEY           hbbs public key (key)
#   RD_PASSWORD      permanent password (controlled) / password to connect with (controller)
#   RD_PEER_ID       controller only: ID of the controlled side
#   RD_LOCAL_OPTS    controller only (also accepted on controlled): space separated
#                    key=value pairs written to RustDesk_local.toml [options],
#                    e.g. "enable-udp-punch=Y enable-webrtc=N"
#   RD_OPTS          space separated key=value pairs added to RustDesk2.toml [options]
#   RD_CONNECT_DELAY seconds to wait before --connect (default 3)
#   RD_GW4/RD_GW6    default gateways ("nat" topology); without RD_GW6 an on-link IPv6
#                    default route is added (RD_V6_DEFAULT_ROUTE=0 disables it)
#
# Status files for the runner: /tmp/rdlab/id (controlled), /tmp/rdlab/ready.
set -u

ROLE="${RD_ROLE:-shell}"
CFG=/root/.config/rustdesk
ST=/tmp/rdlab
mkdir -p "$CFG" "$ST"

log() { echo "[rdlab-client $(date +%H:%M:%S)] $*"; }

if [ -e /usr/share/rustdesk/libsciter-gtk.so ]; then
    VARIANT=sciter
else
    VARIANT=flutter
fi
echo "$VARIANT" > "$ST/variant"
rustdesk --version > "$ST/version" 2>/dev/null || true
log "role=$ROLE variant=$VARIANT version=$(cat "$ST/version" 2>/dev/null)"

toml_options() {
    # $1: space separated key=value list
    local kv k v
    for kv in $1; do
        k="${kv%%=*}"
        v="${kv#*=}"
        printf "%s = '%s'\n" "$k" "$v"
    done
}

write_config() {
    # RustDesk2.toml = Config2 (libs/hbb_common/src/config.rs): server settings live in
    # [options]. Written before the first start, so no process can overwrite it.
    {
        echo "rendezvous_server = ''"
        echo "nat_type = 0"
        echo "serial = 0"
        echo ""
        echo "[options]"
        [ -n "${RD_SERVER:-}" ] && echo "custom-rendezvous-server = '${RD_SERVER}'"
        [ -n "${RD_RELAY:-}" ] && echo "relay-server = '${RD_RELAY}'"
        [ -n "${RD_KEY:-}" ] && echo "key = '${RD_KEY}'"
        toml_options "${RD_OPTS:-}"
    } > "$CFG/RustDesk2.toml"
    # RustDesk_local.toml = LocalConfig: the transport switches enable-*-punch / enable-webrtc
    # are read from here (src/common.rs get_local_option), on the controlling side.
    {
        echo "remote_id = ''"
        echo ""
        echo "[options]"
        toml_options "${RD_LOCAL_OPTS:-}"
    } > "$CFG/RustDesk_local.toml"
    cp "$CFG/RustDesk2.toml" "$CFG/RustDesk_local.toml" "$ST/" 2>/dev/null
}

# The lab network is internal: no default route. Real hosts have one, and the client's
# IPv6 detection (src/common.rs test_bind_ipv6) asks the kernel for a route to
# 2001:4860:4860::8888 to learn its global v6 address *synchronously*; without a route it
# only learns it later via STUN, too late for a --connect started right after launch.
# An on-link default route is enough (nothing is ever sent to it). Needs NET_ADMIN.
add_default_routes() {
    # "nat" topology: the router container is the gateway for both families.
    if [ -n "${RD_GW4:-}" ]; then
        ip route replace default via "$RD_GW4" \
            || log "WARNING: could not set the IPv4 default route via $RD_GW4"
    fi
    if [ -n "${RD_GW6:-}" ]; then
        ip -6 route replace default via "$RD_GW6" metric 1024 \
            || log "WARNING: could not set the IPv6 default route via $RD_GW6"
    elif [ "${RD_V6_DEFAULT_ROUTE:-1}" = 1 ] && ! ip -6 route show default | grep -q dev; then
        ip -6 route add default dev eth0 metric 1024 2>/dev/null \
            || log "WARNING: could not add an IPv6 default route (NET_ADMIN missing?)"
    fi
    { ip route show default; ip -6 route show default; } > "$ST/routes" 2>&1
}

start_x() {
    Xvfb :0 -screen 0 1280x800x24 -nolisten tcp -ac > "$ST/xvfb.log" 2>&1 &
    for _ in $(seq 1 100); do
        [ -S /tmp/.X11-unix/X0 ] && break
        sleep 0.1
    done
    export DISPLAY=:0
    # Session bus for GTK / Flutter (dbus is also how Flutter forwards --connect to a
    # running instance; there is none, so it falls back to handling it itself).
    eval "$(dbus-launch --sh-syntax)"
    export DBUS_SESSION_BUS_ADDRESS
    echo "DISPLAY=$DISPLAY DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS" > "$ST/env"
}

stop_all() {
    log "stopping"
    pkill -TERM -f "rustdesk" 2>/dev/null
    pkill -TERM Xvfb 2>/dev/null
    exit 0
}
trap stop_all TERM INT

add_default_routes

case "$ROLE" in
controlled)
    write_config
    start_x
    log "starting rustdesk --server"
    rustdesk --server > "$ST/server.stdout" 2>&1 &
    SERVER_PID=$!
    # --password and --get-id talk to the --server process over IPC; they need
    # is_installed() (exe under /usr) and root, both true here.
    ok=""
    for _ in $(seq 1 60); do
        out="$(rustdesk --password "${RD_PASSWORD:?}" 2>&1)"
        if echo "$out" | grep -q "Done"; then ok=1; break; fi
        sleep 1
    done
    [ -z "$ok" ] && log "WARNING: setting the permanent password failed: $out"
    id=""
    for _ in $(seq 1 60); do
        id="$(rustdesk --get-id 2>/dev/null | tr -d '[:space:]')"
        [ -n "$id" ] && break
        sleep 1
    done
    log "id=$id password-set=${ok:-no}"
    echo "$id" > "$ST/id"
    touch "$ST/ready"
    wait "$SERVER_PID"
    log "rustdesk --server exited with $?"
    sleep infinity &
    wait $!
    ;;
controller)
    write_config
    start_x
    sleep "${RD_CONNECT_DELAY:-3}"
    log "connecting to ${RD_PEER_ID:?}"
    touch "$ST/ready"
    if [ "$VARIANT" = sciter ]; then
        # sciter: --connect <id> [password] (src/ui.rs: positional password)
        rustdesk --connect "$RD_PEER_ID" "${RD_PASSWORD:-}" > "$ST/connect.stdout" 2>&1 &
    else
        # flutter: --connect <id> --password <pw> (src/core_main.rs core_main_invoke_new_connection)
        rustdesk --connect "$RD_PEER_ID" --password "${RD_PASSWORD:-}" > "$ST/connect.stdout" 2>&1 &
    fi
    wait $!
    log "rustdesk --connect exited with $?"
    sleep infinity &
    wait $!
    ;;
shell)
    write_config
    start_x
    sleep infinity &
    wait $!
    ;;
*)
    exec "$@"
    ;;
esac
