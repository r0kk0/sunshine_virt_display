pub mod framing;

// Re-exports for convenience; consumed by the IPC server in T3.2.
#[allow(unused_imports)]
pub use framing::{read_frame, write_frame, FrameError, MAX_FRAME_SIZE};
