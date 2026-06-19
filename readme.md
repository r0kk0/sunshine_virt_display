# Sunshine Virtual Display

Creates a KWin/Wayland virtual display matching a Sunshine client's resolution and refresh rate. A small privileged daemon injects an EDID through DRM debugfs; compositor operations run as the authenticated desktop user through `kscreen-doctor`.

> Enable SSH before testing display changes. If recovery is needed, run `svd disconnect` or `sudo systemctl stop sunshine-vd`.

## Requirements

- Linux with systemd
- KDE Plasma on Wayland with `kwin_wayland`
- `kscreen-doctor` at `/usr/bin/kscreen-doctor`
- debugfs mounted at `/sys/kernel/debug`
- stable Rust toolchain for source installation

Intel, AMD, and NVIDIA DRM devices are supported. Hyprland, GNOME, HDR, and multiple simultaneous virtual displays are not currently supported.

## Installation

Install for the same desktop user that runs Sunshine:

```bash
sudo ./install.sh --user "$USER"
```

The installer builds release binaries, creates the `sunshine-vd` system group, adds the selected user, installs the hardened systemd unit, and preserves an existing `/etc/sunshine-vd/config.toml`. Log out and back in after the first installation so the new group membership applies.

For distribution packaging without user changes:

```bash
sudo ./install.sh --no-user
```

Start the daemon:

```bash
sudo systemctl enable --now sunshine-vd
systemctl status sunshine-vd
```

## Sunshine Configuration

Configure Sunshine's General → Do/Undo commands:

```bash
# Do
svd connect --width ${SUNSHINE_CLIENT_WIDTH} --height ${SUNSHINE_CLIENT_HEIGHT} --refresh ${SUNSHINE_CLIENT_FPS}

# Undo
svd disconnect
```

The CLI user must own the active Plasma session and belong to `sunshine-vd`. Root is reserved for recovery and administration.

Useful commands:

```bash
svd status
svd status --json
svd connect --width 2560 --height 1440 --refresh 120 --exclusive
svd disconnect
svd restore
```

## Configuration

The optional file `/etc/sunshine-vd/config.toml` accepts only active settings; unknown or unsafe values stop daemon startup.

```toml
device = "card0"                    # optional fixed DRM card
output_ready_timeout_secs = 30       # 1..120
ipc_timeout_secs = 2                 # 1..30
log_level = "info"                   # error|warn|info|debug|trace
disable_outputs = ["DP-1"]          # optional non-exclusive output list

[[extra_allowed_modes]]
width = 2560
height = 1440
refresh = 165
```

`RUST_LOG` overrides `log_level`; `--verbose` selects debug logging when `RUST_LOG` is absent. Socket and state paths are fixed at `/run/sunshine-vd/svd.sock` and `/var/lib/sunshine-vd/state.json`.

## Safety and Recovery

Before changing displays, the daemon stores a versioned, mode-`0600` recovery journal. Lifecycle phases are `disconnected`, `connecting`, `connected`, `disconnecting`, and `recovery_required`; JSON status includes the current phase.

Connect succeeds only after KWin reports the requested output geometry. Any later failure restores physical outputs before disabling the virtual connector and clearing its EDID. If cleanup is incomplete, the journal is retained as `recovery_required` rather than discarded.

Recovery runs automatically at startup. It also runs when Sunshine exits, before sleep, and on SIGTERM/SIGINT. Manual commands are:

```bash
svd restore
svd disconnect
journalctl -u sunshine-vd -n 100 --no-pager
sudo python3 scripts/debug_virt_display.py
```

## Security Model

- The Unix socket is `0660 root:sunshine-vd`; kernel peer credentials identify every client.
- Mutating operations are restricted to root or the owner of the active KWin session.
- IPC frames are limited to 4096 bytes and have bounded read/write timeouts.
- Card and connector identifiers are validated before entering paths or compositor commands.
- KWin subprocesses run with the desktop UID/GID and a minimal environment.
- The systemd unit bounds capabilities and restricts network, filesystem, kernel, and process features.

## Development

```bash
make build
make test
make lint
# or all checks:
make check
```

Rust workspace layout:

- `crates/svd-proto`: validated IPC types and framing
- `crates/svd-cli`: unprivileged `svd` client
- `crates/svd-daemon`: privileged IPC, lifecycle, KWin, DRM, sleep, and watcher logic
- `deploy`: service and configuration examples


## Migrating from 0.1

1. Remove `hdr`, `allow_master_stealing`, `socket_path`, and `state_path` from config.
2. Reinstall with `sudo ./install.sh --user "$USER"` and re-login.
3. Update JSON consumers for the additive `phase` status field.
4. The retired Python daemon and tests are no longer shipped; the diagnostic Python script remains supported.
