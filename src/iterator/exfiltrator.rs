use std::sync::atomic::{AtomicBool, Ordering};

use libc::{c_int, siginfo_t};

mod sealed {
    use libc::{c_int, siginfo_t};

    pub unsafe trait Exfiltrator: Send + Sync + 'static {
        type Storage: Default + Send + Sync + 'static;
        type Output;
        fn supports_signal(&self, sig: c_int) -> bool;
        fn store(&self, slot: &Self::Storage, signal: c_int, info: &siginfo_t);
        fn load(&self, slot: &Self::Storage, signal: c_int) -> Option<Self::Output>;
    }
}

pub trait Exfiltrator: sealed::Exfiltrator { }

impl<E: sealed::Exfiltrator> Exfiltrator for E { }

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
        if slot.swap(false, Ordering::SeqCst) {
            Some(signal)
        } else {
            None
        }
    }
}
