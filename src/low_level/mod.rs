//! Some low level utilities
//!
//! More often to build other abstractions than used directly.

use std::io::Error;

use libc::c_int;

#[cfg(feature = "channel")]
pub mod channel;
#[cfg(not(windows))]
pub mod pipe;

pub use signal_hook_registry::{register, unregister};

/// The usual raise, just the safe wrapper around it.
///
/// This is async-signal-safe.
pub fn raise(sig: c_int) -> Result<(), Error> {
    let result = unsafe { libc::raise(sig) };
    if result == -1 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A bare libc abort.
///
/// Unlike the [std::process::abort], this one is guaranteed to contain no additions or wrappers
/// and therefore is async-signal-safe. You can use this to terminate the application from within a
/// signal handler.
pub fn abort() -> ! {
    unsafe {
        libc::abort();
    }
}
