//! Module for actions setting flags.
//!
//! This contains helper functions to set flags whenever a signal happens. The flags are atomic
//! bools or numbers and the library manipulates them with the `SeqCst` ordering, in case someone
//! cares about relative order to some *other* atomic variables. If you don't care about the
//! relative order, you are free to use `Ordering::Relaxed` when reading and resetting the flags.
//!
//! # When to use
//!
//! The flags in this module allow for polling if a signal arrived since the previous poll. The do
//! not allow blocking until something arrives.
//!
//! Therefore, the natural way to use them is in applications that have some kind of iterative work
//! with both some upper and lower time limit on one iteration. If one iteration could block for
//! arbitrary time, the handling of the signal would be postponed for a long time. If the iteration
//! didn't block at all, the checking for the signal would turn into a busy-loop.
//!
//! If what you need is blocking until a signal comes, you might find better tools in the
//! [`pipe`](../pipe/) and [`iterator`](../iterator/) modules.
//!
//! # Examples
//!
//! Doing something until terminated. This also knows by which signal it was terminated. In case
//! multiple termination signals arrive before it is handled, it recognizes the last one.
//!
//! ```rust
//! use std::io::Error;
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicUsize, Ordering};
//!
//! use signal_hook::consts::signal::*;
//! use signal_hook::flag as signal_flag;
//!
//! fn main() -> Result<(), Error> {
//!     let term = Arc::new(AtomicUsize::new(0));
//!     const SIGTERM_U: usize = SIGTERM as usize;
//!     const SIGINT_U: usize = SIGINT as usize;
//! #   #[cfg(not(windows))]
//!     const SIGQUIT_U: usize = SIGQUIT as usize;
//!     signal_flag::register_usize(SIGTERM, Arc::clone(&term), SIGTERM_U)?;
//!     signal_flag::register_usize(SIGINT, Arc::clone(&term), SIGINT_U)?;
//! #   #[cfg(not(windows))]
//!     signal_flag::register_usize(SIGQUIT, Arc::clone(&term), SIGQUIT_U)?;
//!
//! #   // Hack to terminate the example when run as a doc-test.
//! #   term.store(SIGTERM_U, Ordering::Relaxed);
//!     loop {
//!         match term.load(Ordering::Relaxed) {
//!             0 => {
//!                 // Do some useful stuff here
//!             }
//!             SIGTERM_U => {
//!                 eprintln!("Terminating on the TERM signal");
//!                 break;
//!             }
//!             SIGINT_U => {
//!                 eprintln!("Terminating on the INT signal");
//!                 break;
//!             }
//! #           #[cfg(not(windows))]
//!             SIGQUIT_U => {
//!                 eprintln!("Terminating on the QUIT signal");
//!                 break;
//!             }
//!             _ => unreachable!(),
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! Sending a signal to self and seeing it arrived (not of a practical usage on itself):
//!
//! ```rust
//! use std::io::Error;
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicBool, Ordering};
//! use std::thread;
//! use std::time::Duration;
//!
//! use signal_hook::consts::signal::*;
//! use signal_hook::low_level::raise;
//!
//! fn main() -> Result<(), Error> {
//!     let got = Arc::new(AtomicBool::new(false));
//! #   #[cfg(not(windows))]
//!     signal_hook::flag::register(SIGUSR1, Arc::clone(&got))?;
//! #   #[cfg(windows)]
//! #   signal_hook::flag::register(SIGTERM, Arc::clone(&got))?;
//! #   #[cfg(not(windows))]
//!     raise(SIGUSR1).unwrap();
//! #   #[cfg(windows)]
//! #   raise(SIGTERM).unwrap();
//!     // A sleep here, because it could run the signal handler in another thread and we may not
//!     // see the flag right away. This is still a hack and not guaranteed to work, it is just an
//!     // example!
//!     thread::sleep(Duration::from_secs(1));
//!     assert!(got.load(Ordering::Relaxed));
//!     Ok(())
//! }
//! ```
//!
//! Reloading a configuration on `SIGHUP` (which is a common behaviour of many UNIX daemons,
//! together with reopening the log file).
//!
//! ```rust
//! use std::io::Error;
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicBool, Ordering};
//!
//! use signal_hook::consts::signal::*;
//! use signal_hook::flag as signal_flag;
//!
//! fn main() -> Result<(), Error> {
//!     // We start with true, to load the configuration in the very first iteration too.
//!     let reload = Arc::new(AtomicBool::new(true));
//!     let term = Arc::new(AtomicBool::new(false));
//! #   #[cfg(not(windows))]
//!     signal_flag::register(SIGHUP, Arc::clone(&reload))?;
//!     signal_flag::register(SIGINT, Arc::clone(&term))?;
//!     signal_flag::register(SIGTERM, Arc::clone(&term))?;
//! #   #[cfg(not(windows))]
//!     signal_flag::register(SIGQUIT, Arc::clone(&term))?;
//!     while !term.load(Ordering::Relaxed) {
//!         // Using swap here, not load, to reset it back to false once it is reloaded.
//!         if reload.swap(false, Ordering::Relaxed) {
//!             // Reload the config here
//! #
//! #           // Hiden hack to make the example terminate when run as doc-test. Not part of the
//! #           // real code.
//! #           term.store(true, Ordering::Relaxed);
//!         }
//!         // Serve one request
//!     }
//!     Ok(())
//! }
//! ```

use std::io::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use libc::c_int;

use crate::SigId;

/// Registers an action to set the flag to `true` whenever the given signal arrives.
pub fn register(signal: c_int, flag: Arc<AtomicBool>) -> Result<SigId, Error> {
    // We use SeqCst for two reasons:
    // * Signals should not come very often, so the performance does not really matter.
    // * We promise the order of actions, but setting different atomics with Relaxed or similar
    //   would not guarantee the effective order.
    unsafe { crate::low_level::register(signal, move || flag.store(true, Ordering::SeqCst)) }
}

/// Registers an action to set the flag to the given value whenever the signal arrives.
pub fn register_usize(signal: c_int, flag: Arc<AtomicUsize>, value: usize) -> Result<SigId, Error> {
    unsafe { crate::low_level::register(signal, move || flag.store(value, Ordering::SeqCst)) }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::consts::signal::*;

    fn self_signal() {
        #[cfg(not(windows))]
        const SIG: c_int = SIGUSR1;
        #[cfg(windows)]
        const SIG: c_int = SIGTERM;
        crate::low_level::raise(SIG).unwrap();
    }

    fn wait_flag(flag: &AtomicBool) -> bool {
        let start = Instant::now();
        while !flag.load(Ordering::Relaxed) {
            atomic::spin_loop_hint();
            if Instant::now() - start > Duration::from_secs(1) {
                // We reached a timeout and nothing happened yet.
                // In theory, using timeouts for thread-synchronization tests is wrong, but a
                // second should be enough in practice.
                return false;
            }
        }
        true
    }

    #[test]
    fn register_unregister() {
        // When we register the action, it is active.
        let flag = Arc::new(AtomicBool::new(false));
        #[cfg(not(windows))]
        let signal = register(SIGUSR1, Arc::clone(&flag)).unwrap();
        #[cfg(windows)]
        let signal = register(crate::SIGTERM, Arc::clone(&flag)).unwrap();
        self_signal();
        assert!(wait_flag(&flag));
        // But stops working after it is unregistered.
        assert!(crate::low_level::unregister(signal));
        flag.store(false, Ordering::Relaxed);
        self_signal();
        assert!(!wait_flag(&flag));
        // And the unregistration actually dropped its copy of the Arc
        assert_eq!(1, Arc::strong_count(&flag));
    }
}
