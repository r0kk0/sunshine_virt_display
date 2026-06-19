pub mod kwin;

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub device: Option<String>,
    /// If true, disable all currently-active physical monitors before
    /// connecting the virtual display (remote headless streaming mode).
    pub exclusive: bool,
    /// Authenticated IPC identity. `None` is reserved for internal recovery.
    pub requester_uid: Option<u32>,
    pub requester_pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ConnectResult {
    pub card: String,
    pub connector: String,
    pub mode: String,
}

#[derive(Debug, Clone)]
pub struct StrategyStatus {
    pub phase: svd_proto::LifecyclePhase,
    pub connected: bool,
    pub card: Option<String>,
    pub connector: Option<String>,
    pub mode: Option<String>,
    pub strategy: Option<String>,
}

#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no suitable DRM card found")]
    NoCard,
    #[error("no empty display slot available")]
    NoSlot,
    #[error("kscreen-doctor not found or failed: {0}")]
    KscreenDoctor(String),
    #[error("compositor (KWin) not found")]
    CompositorNotFound,
    #[error("multiple usable KWin compositors found")]
    AmbiguousCompositor,
    #[error("connect timeout waiting for CRTC assignment")]
    Timeout,
    #[error("not connected (no state file)")]
    NotConnected,
    #[error("already connected — disconnect first")]
    AlreadyConnected,
    #[error("requester is not authorized for the active display session")]
    Unauthorized,
    #[error("{0}")]
    Other(String),
}

pub trait DisplayStrategy: Send + Sync {
    fn connect(&self, params: &ConnectParams) -> Result<ConnectResult, StrategyError>;
    fn disconnect(&self) -> Result<(), StrategyError>;
    fn restore(&self) -> Result<(), StrategyError>;
    fn status(&self) -> StrategyStatus;
    fn is_authorized(&self, uid: u32) -> bool;
}
