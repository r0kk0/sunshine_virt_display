# Sunshine Virtual Display

Creates a virtual display that matches your streaming client's resolution and refresh rate when using [Sunshine](https://github.com/LizardByte/Sunshine) on KDE/KWin. The daemon runs as root, manipulates sysfs/debugfs to inject a custom EDID, and uses `kscreen-doctor` to manage outputs cooperatively through the compositor — no DRM master stealing.

> ⚠️ **Enable SSH before using this.** If your display gets stuck, recover via `sudo svd disconnect` or `sudo systemctl stop sunshine-vd`.

---

## Requirements

- KDE Plasma with KWin (Wayland session)
- `kscreen-doctor` in PATH (comes with KDE, verify with `kscreen-doctor --version`)
- `debugfs` mounted at `/sys/kernel/debug/` (standard on most distros)
- Rust toolchain (`cargo`)
- systemd

Works on Intel, AMD, and NVIDIA GPUs.

---

## Installation

```bash
git clone https://github.com/frostplexx/sunshine_virt_display
cd sunshine_virt_display
sudo ./install.sh          # builds release binaries + installs systemd service
```

The installer:
1. Builds `svd-daemon` and `svd` in release mode
2. Copies them to `/usr/local/bin/`
3. Installs `deploy/sunshine-vd.service` to `/etc/systemd/system/`
4. Runs `systemctl daemon-reload`

Enable and start the daemon:

```bash
sudo systemctl enable --now sunshine-vd
systemctl status sunshine-vd
journalctl -u sunshine-vd -f
```

### Updating

```bash
git pull
sudo ./install.sh
sudo systemctl restart sunshine-vd
```

---

## Configure Sunshine

The daemon listens on `/run/sunshine-vd/svd.sock`. Configure Sunshine's **Do/Undo commands** in the **General** tab:

**Do Command (On Client Connect):**
```bash
svd connect --width ${SUNSHINE_CLIENT_WIDTH} --height ${SUNSHINE_CLIENT_HEIGHT} --refresh ${SUNSHINE_CLIENT_FPS}
```

**Undo Command (On Client Disconnect):**
```bash
svd disconnect
```

`svd` must be in PATH for the user running Sunshine, or use the full path `/usr/local/bin/svd`.

### Multi-GPU systems

The daemon automatically picks the GPU with the most connected displays. To force a specific card:

```bash
svd connect --width 1920 --height 1080 --refresh 60 --device card1
```

---

## Manual Usage

```bash
# Check daemon status
svd status

# Connect a virtual display
sudo svd connect --width 1920 --height 1080 --refresh 60

# Disconnect
sudo svd disconnect

# Restore state after daemon restart
sudo svd restore

# JSON output (for scripting)
svd status --json
```

`svd` sends commands to the daemon over the socket. The daemon must be running. Most operations (connect, disconnect) require the daemon to run as root, but the CLI itself can be run as any user that can reach the socket.

---

## Configuration

Optional config file at `/etc/sunshine-vd/config.toml`:

```toml
# Seconds to wait for KWin to assign a CRTC to the virtual display (default: 30)
output_ready_timeout_secs = 30

# Additional resolutions beyond the built-in VIC table
# [[extra_allowed_modes]]
# width = 2560
# height = 1440
# refresh = 165

# Force a specific DRM device (default: auto-select the card with most displays)
# device = "card0"

# Log level: "error", "warn", "info", "debug", "trace" (default: "info")
log_level = "info"
```

Common resolutions (1080p, 1440p, 4K at standard refresh rates) are in the built-in VIC table. Add `extra_allowed_modes` for non-standard resolutions.

---

## What Happens Automatically

### On connect

1. Finds `kwin_wayland` PID, reads its session environment (`WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`)
2. Generates a custom EDID matching the requested resolution/refresh
3. Finds the first free DP or HDMI connector on the most-connected GPU
4. Writes the EDID override via debugfs (`/sys/kernel/debug/dri/N/PORT/edid_override`)
5. Clears stale KWin output config for that port
6. Disables physical outputs via `kscreen-doctor output.X.disable`
7. Enables the virtual connector via sysfs (`echo on > /sys/class/drm/cardN-PORT/status`)
8. Waits up to `output_ready_timeout_secs` for KWin to assign a CRTC automatically
9. If KWin doesn't assign: forces the mode via `kscreen-doctor output.PORT.mode.WxH@R`
10. Saves state to `/var/lib/sunshine-vd/state.json`
11. Spawns a crash-watcher thread monitoring the `sunshine` PID via `pidfd`

### On disconnect

1. Re-enables physical outputs via `kscreen-doctor output.X.enable`
2. Disables virtual connector: `kscreen-doctor output.PORT.disable` + sysfs `echo off`
3. Clears the EDID override
4. Deletes the state file

### If Sunshine crashes

The crash-watcher thread detects Sunshine's exit via `pidfd_open` + `poll`. It automatically calls disconnect so your physical monitors come back without manual intervention.

### On system sleep

The daemon holds a systemd logind **sleep inhibitor delay lock** so the system waits for it before suspending. When sleep is triggered:
1. Daemon disconnects the virtual display
2. Releases the inhibitor (system proceeds to sleep)

On wake, the inhibitor is re-acquired for the next sleep cycle. **Note**: automatic reconnect after wake is not yet implemented — after waking up, re-run `svd connect` or trigger Sunshine's Do Command again.

### On shutdown / SIGTERM

The daemon disconnects the virtual display before exiting, ensuring physical monitors are always restored even if Sunshine's Undo Command didn't run.

---

## Troubleshooting

**`svd status` says "daemon not running"**
```bash
sudo systemctl start sunshine-vd
journalctl -u sunshine-vd -n 50
```

**Connect fails with "compositor not found"**
- KWin must be running in a Wayland session
- Check: `pgrep -a kwin_wayland`

**Connect fails with "kscreen-doctor not found"**
- Install KDE Plasma tools: `sudo pacman -S plasma-workspace` (Arch) or equivalent

**Physical monitors don't come back after disconnect**
```bash
sudo svd disconnect   # try again; it's idempotent
# Or manually:
sudo systemctl restart sunshine-vd
```

**State file is stale after daemon crash**
```bash
sudo svd restore      # loads persisted state so disconnect works again
sudo svd disconnect
```

**Virtual display not assigned a mode**
- Increase `output_ready_timeout_secs` in config
- Verify: `kscreen-doctor -o` (should show the new virtual output)

**Daemon logs**
```bash
journalctl -u sunshine-vd -f          # follow logs
journalctl -u sunshine-vd --since "5 min ago"
# Enable debug logging:
RUST_LOG=debug sudo svd-daemon --verbose
```

---

## Known Limitations

- **Reconnect after wake**: not automatic — must re-run connect manually
- **KDE only**: no Hyprland, no GNOME support in this version
- **HDR**: not implemented (EDID 1.4 base only)
- **Non-standard resolutions**: resolutions not in the VIC table must be added to `extra_allowed_modes` in config
- **Single virtual display at a time**: only one virtual display can be connected simultaneously

---

## How the daemon is structured

```
svd-daemon
├── ipc/         Unix socket server (newline-delimited JSON)
├── config.rs    TOML config with safe defaults
├── handler.rs   RealHandler: translates IPC requests → strategy calls
├── watcher.rs   Sunshine crash watcher (pidfd + poll)
├── sleep.rs     Logind D-Bus sleep/wake handler + inhibitor
└── strategy/
    └── kwin/
        ├── mod.rs      KWinStrategy (full DisplayStrategy impl)
        ├── env.rs      Finds kwin_wayland PID, reads /proc environ
        ├── edid.rs     EDID 1.4 generator
        ├── sysfs.rs    /sys/class/drm + debugfs I/O
        ├── kscreen.rs  kscreen-doctor subprocess wrapper
        └── state.rs    JSON state persistence (atomic write)
```

`svd` (CLI) connects to the daemon socket, sends the request, and prints the response. All the logic is in the daemon.
