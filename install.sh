#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: sudo ./install.sh (--user USER | --no-user) [--debug]

  --user USER  Add the desktop/Sunshine user to the sunshine-vd control group.
  --no-user    Install without changing group membership (for package builders).
  --debug      Install debug binaries; a release build is the default.
  --help       Show this help.
EOF
}

build_mode=release
install_user=
no_user=false

while (($#)); do
    case "$1" in
        --user)
            [[ $# -ge 2 ]] || { echo "--user requires a value" >&2; exit 2; }
            [[ -z "$install_user" ]] || { echo "--user may be specified once" >&2; exit 2; }
            install_user=$2
            shift 2
            ;;
        --no-user)
            no_user=true
            shift
            ;;
        --debug)
            build_mode=debug
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -n "$install_user" && "$no_user" == true ]]; then
    echo "--user and --no-user are mutually exclusive" >&2
    exit 2
fi
if [[ -z "$install_user" && "$no_user" == false ]]; then
    echo "choose --user USER or --no-user" >&2
    exit 2
fi
if ((EUID != 0)); then
    echo "installation must run as root" >&2
    exit 1
fi

if [[ "$build_mode" == release ]]; then
    cargo build --workspace --release
    bin_dir=target/release
else
    cargo build --workspace
    bin_dir=target/debug
fi

if ! getent group sunshine-vd >/dev/null; then
    groupadd --system sunshine-vd
fi
if [[ -n "$install_user" ]]; then
    id "$install_user" >/dev/null
    usermod --append --groups sunshine-vd "$install_user"
fi

install -m 755 "$bin_dir/svd-daemon" /usr/local/bin/svd-daemon
install -m 755 "$bin_dir/svd" /usr/local/bin/svd
install -m 755 "$bin_dir/svd-restore" /usr/local/bin/svd-restore
install -m 644 deploy/sunshine-vd.service /etc/systemd/system/sunshine-vd.service
install -d -m 750 /etc/sunshine-vd
if [[ ! -e /etc/sunshine-vd/config.toml ]]; then
    install -m 640 deploy/config.toml.example /etc/sunshine-vd/config.toml
fi
systemctl daemon-reload

echo "Installed sunshine-vd ($build_mode) and reloaded systemd."
if [[ -n "$install_user" ]]; then
    echo "Added $install_user to sunshine-vd; log out and back in before using svd."
fi
echo "Run: systemctl enable --now sunshine-vd"
