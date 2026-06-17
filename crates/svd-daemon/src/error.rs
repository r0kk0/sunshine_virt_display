/// Error types for svd-daemon.
///
/// Variants marked `#[allow(dead_code)]` below are reserved for future
/// milestones (M2-M8) and are intentionally not constructed yet.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum DaemonError {
    /// Wraps any std::io::Error via the From trait.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Configuration parsing / validation failure.
    #[error("configuration error: {0}")]
    Config(String),

    /// IPC channel failure.
    #[error("IPC error: {0}")]
    Ipc(String),

    /// sysfs interaction failure.
    #[error("sysfs error: {0}")]
    Sysfs(String),

    /// DRM/KMS operation failure.
    #[error("DRM error: {0}")]
    Drm(String),

    /// Wayland/X11 compositor interaction failure.
    #[error("compositor error: {0}")]
    Compositor(String),

    /// No safe strategy is available to satisfy the request.
    #[error("no safe strategy available: {0}")]
    NoSafeStrategy(String),

    /// Master-stealing is required but has not been enabled in config.
    #[error("set allow_master_stealing in config to enable")]
    MasterStealingDisabled,
}
