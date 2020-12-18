//! Some low level utilities
//!
//! More often to build other abstractions than used directly.

#[cfg(feature = "channel")]
pub mod channel;
#[cfg(not(windows))]
pub mod pipe;

pub use signal_hook_registry::{register, unregister};
