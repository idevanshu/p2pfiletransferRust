#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────
#  p2pfiletransfer — launcher (Linux / macOS / WSL)
#
#  Usage:
#    ./start.sh                          Interactive mode
#    ./start.sh send <file-or-folder>    Send directly
#    ./start.sh receive <address>        Receive directly
#
#  Options (append after file/address):
#    --relay <multiaddr>    Use relay for NAT traversal
#    --port <port>          Listen on specific port
# ──────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── find binary ───────────────────────────────────────────

find_binary() {
    # 1. release build in project
    local rel="$SCRIPT_DIR/target/release/p2pfiletransfer"
    [ -x "$rel" ] && echo "$rel" && return

    # 2. debug build in project
    local dbg="$SCRIPT_DIR/target/debug/p2pfiletransfer"
    [ -x "$dbg" ] && echo "$dbg" && return

    # 3. installed in PATH
    if command -v p2pfiletransfer &>/dev/null; then
        command -v p2pfiletransfer
        return
    fi

    echo ""
}

BIN="$(find_binary)"
if [ -z "$BIN" ]; then
    echo "p2pfiletransfer binary not found."
    echo "run ./setup.sh first to build the project."
    exit 1
fi

# ── direct mode (arguments passed) ────────────────────────

if [ $# -ge 1 ]; then
    case "$1" in
        send)
            shift
            if [ $# -lt 1 ]; then
                echo "usage: ./start.sh send <file-or-folder> [--relay ADDR] [--port PORT]"
                exit 1
            fi
            FILE="$1"; shift
            exec "$BIN" -m send -f "$FILE" "$@"
            ;;
        receive|recv)
            shift
            if [ $# -lt 1 ]; then
                echo "usage: ./start.sh receive <sender-address> [--relay ADDR] [--port PORT]"
                exit 1
            fi
            ADDR="$1"; shift
            exec "$BIN" -m receive -a "$ADDR" "$@"
            ;;
        -h|--help|help)
            exec "$BIN" --help
            ;;
        *)
            echo "unknown command: $1"
            echo "usage: ./start.sh [send|receive|help]"
            exit 1
            ;;
    esac
fi

# ── interactive mode ─────────────────────────────────────

echo ""
echo "══════════════════════════════════════════"
echo "  p2pfiletransfer"
echo "══════════════════════════════════════════"
echo ""
echo "  1) Send a file or folder"
echo "  2) Receive a file or folder"
echo "  3) Show help"
echo "  4) Exit"
echo ""

read -rp "choose [1-4]: " choice

case "$choice" in
    1)
        echo ""
        read -rp "path to file or folder: " filepath
        if [ -z "$filepath" ]; then
            echo "no path given."; exit 1
        fi
        if [ ! -e "$filepath" ]; then
            echo "path not found: $filepath"; exit 1
        fi

        echo ""
        read -rp "use a relay server? [y/N]: " use_relay
        RELAY_ARGS=""
        if [[ "$use_relay" =~ ^[yY] ]]; then
            read -rp "relay multiaddr: " relay_addr
            RELAY_ARGS="-r $relay_addr"
        fi

        read -rp "listen port (enter for random): " port
        PORT_ARGS=""
        if [ -n "$port" ]; then
            PORT_ARGS="-p $port"
        fi

        echo ""
        echo "starting sender..."
        echo "──────────────────────────────────────────"
        # shellcheck disable=SC2086
        exec "$BIN" -m send -f "$filepath" $RELAY_ARGS $PORT_ARGS
        ;;
    2)
        echo ""
        read -rp "sender address (/ip4/.../p2p/...): " address
        if [ -z "$address" ]; then
            echo "no address given."; exit 1
        fi

        read -rp "use a relay server? [y/N]: " use_relay
        RELAY_ARGS=""
        if [[ "$use_relay" =~ ^[yY] ]]; then
            read -rp "relay multiaddr: " relay_addr
            RELAY_ARGS="-r $relay_addr"
        fi

        read -rp "listen port (enter for random): " port
        PORT_ARGS=""
        if [ -n "$port" ]; then
            PORT_ARGS="-p $port"
        fi

        echo ""
        echo "connecting to sender..."
        echo "──────────────────────────────────────────"
        # shellcheck disable=SC2086
        exec "$BIN" -m receive -a "$address" $RELAY_ARGS $PORT_ARGS
        ;;
    3)
        exec "$BIN" --help
        ;;
    4)
        echo "bye."
        exit 0
        ;;
    *)
        echo "invalid choice."; exit 1
        ;;
esac
