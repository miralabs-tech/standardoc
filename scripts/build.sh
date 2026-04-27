#!/usr/bin/env bash
# Interactive build helper for standardoc-server.
#
# - dev     : builds to target-dev/ — never conflicts with running servers
# - prod    : kills running standardoc-server, then builds to target/
# - inspect : lists running servers (pid, parent, args)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

list_servers() {
    if command -v pgrep >/dev/null 2>&1; then
        pgrep -af 'standardoc-server' || true
    else
        ps -ef | grep '[s]tandardoc-server' || true
    fi
}

show_servers() {
    local out
    out="$(list_servers)"
    if [[ -z "$out" ]]; then
        echo "No standardoc-server processes running."
        return 1
    fi
    echo
    echo "Running standardoc-server processes:"
    echo "$out"
    return 0
}

dev_build() {
    echo
    echo "Building release into target-dev/ (no kill required)..."
    cargo build --release --features standardoc-web/embedded-frontend --features standardoc-web/embedded-frontend --target-dir target-dev
    echo
    echo "Built: $REPO_ROOT/target-dev/release/standardoc-server"
}

prod_build() {
    if show_servers; then
        echo
        read -r -p "Kill these processes and rebuild? (y/N) " confirm
        if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
            echo "Aborted."
            return
        fi
        pkill -f standardoc-server || true
        sleep 1
    fi
    echo
    echo "Building release into target/..."
    cargo build --release --features standardoc-web/embedded-frontend
    echo
    echo "Built: $REPO_ROOT/target/release/standardoc-server"
    echo "Restart your IDE(s) to spawn fresh servers."
}

while true; do
    echo
    echo "standardoc build helper"
    echo "  [1] dev      build to target-dev/ (parallel-safe)"
    echo "  [2] prod     kill servers + build to target/"
    echo "  [3] inspect  list running servers, no build"
    echo "  [q] quit"
    echo
    read -r -p "Choice: " choice

    case "$choice" in
        1) dev_build ;;
        2) prod_build ;;
        3) show_servers || true ;;
        q|Q) exit 0 ;;
        *) echo "Unknown choice: $choice" >&2 ;;
    esac
done
