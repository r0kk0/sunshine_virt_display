#!/usr/bin/env bash
set -euo pipefail

RELEASE=${1:-debug}

if [[ "$RELEASE" == "release" ]]; then
    cargo build --release
    BIN_DIR=target/release
else
    cargo build
    BIN_DIR=target/debug
fi

install -m 755 "$BIN_DIR/svd-daemon" /usr/local/bin/svd-daemon
install -m 755 "$BIN_DIR/svd" /usr/local/bin/svd
install -m 644 deploy/sunshine-vd.service /etc/systemd/system/sunshine-vd.service
systemctl daemon-reload

echo "Installed svd-daemon and svd to /usr/local/bin/"
echo "Service file installed to /etc/systemd/system/sunshine-vd.service"
echo "Run: systemctl enable --now sunshine-vd"
