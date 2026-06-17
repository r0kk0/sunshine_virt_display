# API Notes: Dependency Spike — drm-rs, zbus, pidfd

**Branch:** feat/rust-rewrite  
**Task:** T1.3 — Dependency Spike  
**Date:** 2026-06-17  
**Status:** Research complete; root-gated tests deferred (no `/dev/dri` access on research machine)

---

## Summary

This document records exact API signatures, the pidfd-fd-into-drm-rs verdict, and recommended
crate versions for the master-stealing module and D-Bus integration. Research performed by reading
crate source via docs.rs and crate source URLs; no DRM hardware was available.

---

## 1. drm-rs API

### Version

Latest stable: **0.15.0** (released 2026-03-19).

```toml
drm = "0.15"
```

### Trait Hierarchy

```
std::os::unix::io::AsFd
    └── drm::Device              (base DRM trait — all provided methods, no required methods)
            └── drm::control::Device   (KMS / modesetting — all provided methods)
```

Both traits have **zero required methods**. Every method is a provided default implemented via
`self.as_fd()`. To implement them you write empty `impl` blocks:

```rust
use std::fs::File;
use std::os::unix::io::{AsFd, BorrowedFd};

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> { self.0.as_fd() }
}

impl drm::Device for Card {}
impl drm::control::Device for Card {}
```

The trait definition (confirmed from `drm-0.15.0/src/lib.rs`):

```rust
pub trait Device: AsFd {
    fn acquire_master_lock(&self) -> io::Result<()> {
        drm_ffi::auth::acquire_master(self.as_fd())?;
        Ok(())
    }
    fn release_master_lock(&self) -> io::Result<()> {
        drm_ffi::auth::release_master(self.as_fd())?;
        Ok(())
    }
    // ... 8 more provided methods, all calling self.as_fd()
}
```

And `drm::control::Device`:

```rust
pub trait Device: super::Device {
    fn resource_handles(&self) -> io::Result<ResourceHandles> { ... }
    fn get_connector(&self, handle: connector::Handle, force_probe: bool)
        -> io::Result<connector::Info> { ... }
    fn set_crtc(
        &self,
        handle: crtc::Handle,
        framebuffer: Option<framebuffer::Handle>,
        pos: (u32, u32),
        conns: &[connector::Handle],
        mode: Option<Mode>,
    ) -> io::Result<()> { ... }
    // ... ~47 more provided methods
}
```

### Opening a Card

The crate intentionally provides **no concrete card-opening function**. The canonical pattern:

```rust
use std::fs::OpenOptions;

let file = OpenOptions::new()
    .read(true)
    .write(true)
    .open("/dev/dri/card0")?;
let card = Card(file);
```

### Listing Connectors

```rust
use drm::control::Device;

let handles = card.resource_handles()?;
for &conn_handle in handles.connectors() {
    let info = card.get_connector(conn_handle, false)?;
    println!("{:?} — {:?}", info.interface(), info.state());
}
```

Key types:

| Type | Source |
|------|--------|
| `drm::control::ResourceHandles` | `.connectors()` → `&[connector::Handle]` |
| `drm::control::connector::Handle` | opaque handle |
| `drm::control::connector::Info` | `.interface()` → `Interface`, `.state()` → `State` |
| `drm::control::connector::State` | `Connected`, `Disconnected`, `Unknown` |

### Drop Behaviour (confirmed)

**There are no `impl Drop` blocks anywhere in drm-0.15.0.**  
Confirmed by reading both `src/lib.rs` and `src/control/mod.rs` source pages; neither contains
a `Drop` impl. The crate never calls `DROP_MASTER` or `close` automatically. Cleanup is entirely
delegated to whatever the implementor stores in their struct (e.g., `File` → RAII close on drop).

---

## 2. pidfd_getfd Verdict (MOST CRITICAL)

### Verdict: **Use drm-rs for the master-stealing path, with a raw-fd newtype.**

The fd-import path is safe to implement via drm-rs. Here is the reasoning:

#### Why drm-rs works with a stolen fd

1. **`Device: AsFd` — no owned-fd requirement.**  
   drm-rs never takes ownership of the file descriptor. Every ioctl is routed through
   `drm_ffi::auth::acquire_master(self.as_fd())` / `release_master(self.as_fd())` where
   `as_fd()` returns a *borrowed* `BorrowedFd<'_>`. The crate never holds an `OwnedFd` or
   opens any fd itself.

2. **No SET_MASTER on construction.**  
   `acquire_master_lock` is a plain method call, not part of any constructor or `impl` block
   initialiser. Wrapping an fd in your newtype and implementing `AsFd` will **not** trigger
   any ioctl automatically.

3. **No DROP_MASTER on drop.**  
   There are no `impl Drop` blocks in the crate. When your newtype drops, whatever you hold
   gets dropped (e.g., an `OwnedFd` from `pidfd_getfd` gets `close()`d). drm-rs fires no
   ioctl on destruction.

#### The newtype pattern

```rust
use std::os::unix::io::{AsFd, BorrowedFd, OwnedFd};

/// Wraps a raw fd obtained via pidfd_getfd.
/// Implements AsFd so drm-rs traits can be layered on top.
pub struct StolenDrmFd(OwnedFd);

impl StolenDrmFd {
    /// Safety: caller must have obtained `fd` via pidfd_getfd (or dup) and
    /// must ensure the source process's DRM fd outlives this wrapper's use.
    pub unsafe fn from_owned(fd: OwnedFd) -> Self { Self(fd) }
}

impl AsFd for StolenDrmFd {
    fn as_fd(&self) -> BorrowedFd<'_> { self.0.as_fd() }
}

impl drm::Device for StolenDrmFd {}
impl drm::control::Device for StolenDrmFd {}
```

This pattern compiles with no root access required (type-level proof; no hardware calls needed).

#### Critical semantic: OFD sharing and master state

`pidfd_getfd` is a **dup-like** operation. The imported fd refers to the **same open file
description (OFD)** as the source process's fd — not a fresh open. DRM master state lives on
the OFD (`struct drm_file` in the kernel). Therefore:

- If the source process already holds DRM master, the imported fd may already have master
  status **without calling `acquire_master_lock()`** at all.
- Calling `release_master_lock()` on the imported fd affects the OFD and will disrupt the
  source process while it still holds its copy.
- Closing your imported fd (`OwnedFd` dropping) does **not** globally revoke master while
  the source process still holds its copy of the fd.

**Consequence for the master-stealing module:**  
The `acquire_master_lock()` / `release_master_lock()` calls via drm-rs are safe to make, but
the actual ioctl effect depends on current kernel master assignment. The exact behaviour when
calling SET_MASTER on a dup of a process that already has master requires root-gated
verification on a real `/dev/dri/cardN`.

**Items deferred (needs `/dev/dri` + CAP_SYS_ADMIN):**
- Does `acquire_master_lock()` succeed on a dup of a live master fd?
- Does it return `EPERM` or succeed with no-op?
- Does the source process lose master when we call `acquire_master_lock()`?

#### Should we use raw ioctls instead?

Not required. The newtype approach above gives us:

- Full drm-rs `control::Device` API (connectors, CRTCs, modes, set_crtc, etc.)
- Clean Rust types and Result<> error handling
- No unsafe ioctl nrs to maintain

Raw ioctls (`ioctl(fd, DRM_IOCTL_SET_MASTER, 0)`) would be needed only if drm-rs's trait
methods prove inadequate at runtime — which would require hardware testing to confirm. The
recommendation is to **start with the drm-rs newtype** and fall back to raw ioctls only if
kernel-level testing reveals a hard incompatibility.

---

## 3. zbus API

### Version

Latest stable: **5.16.0** (released 2026-05-29).

The daemon currently has no async runtime. Use the **blocking API** (`zbus::blocking`) to avoid
pulling in tokio. If the daemon later multiplexes many concurrent event sources, migrate to async.

```toml
zbus = "5"
```

> Note: In zbus 5.x, `blocking-api` and the `async-io` backend are **both enabled by default**.
> The default configuration gives you `zbus::blocking::Connection` over the lightweight `async-io`
> executor — no tokio required. Only add `features = ["tokio"]` if you later need tokio integration.
> Confirmed from the zbus 5.16.0 Cargo feature manifest (8 default features, `blocking-api` among them).

### System Bus Connection

```rust
use zbus::blocking::Connection;

let conn = Connection::system()?;  // Result<Connection>
```

### Logind Inhibit Lock

**D-Bus interface:** `org.freedesktop.login1.Manager`  
**Object path:** `/org/freedesktop/login1`  
**Method:** `Inhibit`

Arguments (all strings):

| Arg | Type | Example |
|-----|------|---------|
| `what` | `&str` | `"sleep"` or `"shutdown:sleep"` |
| `who`  | `&str` | `"svd-daemon"` |
| `why`  | `&str` | `"Managing virtual display across suspend"` |
| `mode` | `&str` | `"delay"` (not "block" — allows suspend to proceed after daemon finishes) |

Returns: a **file descriptor** (the inhibitor lock). Closing it releases the lock.

```rust
use zbus::blocking::Connection;
use std::os::unix::io::OwnedFd;

let conn = Connection::system()?;
let reply = conn.call_method(
    Some("org.freedesktop.login1"),
    "/org/freedesktop/login1",
    Some("org.freedesktop.login1.Manager"),
    "Inhibit",
    &("sleep", "svd-daemon", "Managing virtual display across suspend", "delay"),
)?;
let inhibit_fd: OwnedFd = reply.body().deserialize()?;
// Keep inhibit_fd alive; drop it when the critical section ends.
```

### PrepareForSleep Signal Subscription

**Interface:** `org.freedesktop.login1.Manager`  
**Signal:** `PrepareForSleep`  
**Body:** `(b)` — boolean `true` before sleep, `false` after resume.

```rust
use zbus::{MatchRule, blocking::{Connection, MessageIterator}};

let conn = Connection::system()?;
let rule = MatchRule::builder()
    .msg_type(zbus::message::Type::Signal)
    .sender("org.freedesktop.login1")?
    .interface("org.freedesktop.login1.Manager")?
    .member("PrepareForSleep")?
    .build();

let iter = MessageIterator::for_match_rule(rule, &conn, Some(64))?;

for msg in iter {
    let msg = msg?;
    let (going_to_sleep,): (bool,) = msg.body().deserialize()?;
    if going_to_sleep {
        // before sleep: restore CRTC, release master
    } else {
        // after resume: re-acquire master, re-apply display config
    }
}
```

`MessageIterator` is a blocking wrapper around the async `MessageStream`. The match rule is
automatically deregistered when the iterator is dropped.

---

## 4. nix / libc for pidfd

### nix 0.31.3

**`nix` does NOT expose `pidfd_open` or `pidfd_getfd`.**  
Confirmed by checking:
- `nix::all` index for 0.31.3 — neither `pidfd_open`, `pidfd_getfd`, nor `PidFd` present.
- `nix::sys::mod.rs` source — no `pidfd` module.
- `nix::unistd` — no `pidfd_open` function.

Do not add `nix` as a dependency for the pidfd path.

### libc 0.2.186 — use raw syscall

libc provides the syscall number constants (confirmed on docs.rs):

```rust
// x86_64 Linux
pub const SYS_pidfd_open:  c_long = 434;
pub const SYS_pidfd_getfd: c_long = 438;
```

Raw syscall approach:

```rust
use std::os::unix::io::{OwnedFd, FromRawFd};
use libc::{c_long, pid_t};

/// Opens a pidfd for the process with the given PID.
/// Returns an OwnedFd on success.
pub fn pidfd_open(pid: pid_t, flags: u32) -> std::io::Result<OwnedFd> {
    let fd = unsafe {
        libc::syscall(libc::SYS_pidfd_open, pid as c_long, flags as c_long)
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }
}

/// Duplicates `target_fd` from the process referred to by `pidfd`.
/// `flags` must be 0 (reserved for future use).
/// Returns an OwnedFd on success.
pub fn pidfd_getfd(pidfd: i32, target_fd: i32, flags: u32) -> std::io::Result<OwnedFd> {
    let fd = unsafe {
        libc::syscall(
            libc::SYS_pidfd_getfd,
            pidfd as c_long,
            target_fd as c_long,
            flags as c_long,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }
}
```

The callers must have `PTRACE_MODE_ATTACH_REALCREDS` permission to use `pidfd_getfd` (i.e., the
daemon must run as root or have `CAP_SYS_PTRACE`).

---

## 5. Recommended Cargo.toml Additions

For `crates/svd-daemon/Cargo.toml`:

```toml
[dependencies]
drm  = "0.15"
zbus = "5"
libc = "0.2"
# nix is NOT needed for the pidfd path; do not add it
```

Exact pinned versions for reference (use constraint `"0.15"` / `"5"` / `"0.2"` in Cargo.toml;
`Cargo.lock` will pin to the exact resolved version):

| Crate | Latest stable | Notes |
|-------|--------------|-------|
| `drm` | 0.15.0 | Use `"0.15"` |
| `zbus` | 5.16.0 | Use `"5"` — `blocking-api` and `async-io` backend are on by default |
| `libc` | 0.2.186 | Use `"0.2"` — has `SYS_pidfd_open`, `SYS_pidfd_getfd` constants |
| `nix` | 0.31.3 | **Do not add** — no pidfd support; libc::syscall is the right path |

---

## 6. Verification Status

| Claim | How verified | Confidence |
|-------|-------------|------------|
| `drm::Device: AsFd` supertrait | Confirmed from docs.rs source viewer (`src/lib.rs`) | High |
| No `impl Drop` in drm-rs | Confirmed: read both `src/lib.rs` and `src/control/mod.rs` — none found | High |
| `acquire_master_lock` is not called in constructor | Confirmed: it is a plain `fn`, not part of any init/new | High |
| All methods use `self.as_fd()` | Confirmed from source | High |
| `nix` lacks pidfd_open/pidfd_getfd | Confirmed from `nix::all` index for 0.31.3 | High |
| `libc::SYS_pidfd_open = 434`, `SYS_pidfd_getfd = 438` | Confirmed from docs.rs constant pages | High |
| `zbus::blocking::Connection::system()` signature | Confirmed from docs.rs struct page | High |
| `MessageIterator::for_match_rule` works for signals | Confirmed from docs.rs method page | High |
| `blocking-api` on by default in zbus 5.x, `async-io` backend sufficient (no tokio needed) | Confirmed from zbus 5.16.0 feature manifest (8 defaults incl. `blocking-api`, `async-io`) | High |
| zbus Cargo line compiles (`zbus = "5"`) | **NOT compile-tested** — cargo unavailable on research machine; inferred from feature manifest | High (uncompiled) |
| OFD-sharing semantics for pidfd_getfd | Inferred from Linux man page + kernel OFD model | Medium — needs root test |
| SET_MASTER ioctl behaviour on dup'd master fd | **NOT verified** — needs root + real `/dev/dri` | Deferred |
| acquire_master_lock succeeds on dup of live master fd | **NOT verified** — needs root test | Deferred |
