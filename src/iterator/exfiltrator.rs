//! An abstraction over exfiltrating information out of signal handlers.
//!
//! The [`Exfiltrator`] trait provides a way to abstract the information extracted from a signal
//! handler and the way it is extracted out of it.
//!
//! The implementations can be used to parametrize the
//! [`SignalsInfo`][crate::iterator::SignalsInfo] to specify what results are returned.
//!
//! # Sealed
//!
//! Currently, the trait is sealed and all methods hidden. This is likely temporary, until some
//! experience with them is gained.

use std::sync::atomic::{AtomicBool, Ordering};

use libc::{c_int, siginfo_t};

mod sealed {
    use std::fmt::Debug;

    use libc::{c_int, siginfo_t};

    pub unsafe trait Exfiltrator: Debug + Send + Sync + 'static {
        type Storage: Debug + Default + Send + Sync + 'static;
        type Output;
        fn supports_signal(&self, sig: c_int) -> bool;
        fn store(&self, slot: &Self::Storage, signal: c_int, info: &siginfo_t);
        fn load(&self, slot: &Self::Storage, signal: c_int) -> Option<Self::Output>;
    }
}

/// A trait describing what and how is extracted from signal handlers.
///
/// By choosing a specific implementor as the type parameter for
/// [`SignalsInfo`][crate::iterator::SignalsInfo], one can pick how much and what information is
/// returned from the iterator.
pub trait Exfiltrator: sealed::Exfiltrator {}

impl<E: sealed::Exfiltrator> Exfiltrator for E {}

/// An [`Exfiltrator`] providing just the signal numbers.
///
/// This is the basic exfiltrator for most needs. For that reason, there's the
/// [`crate::iterator::Signals`] type alias, to simplify the type names for usual needs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SignalOnly;

unsafe impl sealed::Exfiltrator for SignalOnly {
    type Storage = AtomicBool;
    fn supports_signal(&self, _: c_int) -> bool {
        true
    }
    type Output = c_int;

    fn store(&self, slot: &Self::Storage, _: c_int, _: &siginfo_t) {
        slot.store(true, Ordering::SeqCst);
    }

    fn load(&self, slot: &Self::Storage, signal: c_int) -> Option<Self::Output> {
        if slot
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            Some(signal)
        } else {
            None
        }
    }
}
