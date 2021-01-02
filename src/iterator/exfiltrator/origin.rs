//! An exfiltrator providing the process that caused the signal.
//!
//! The [`WithOrigin`] is an [`Exfiltrator`][crate::iterator::exfiltrator::Exfiltrator] that
//! provides the information about sending process in addition to the signal number, through the
//! [`Origin`] type.
//!
//! See the [`WithOrigin`] example.

// Note on unsafety in this module:
// * Implementing an unsafe trait, that one needs to ensure at least store is async-signal-safe.
//   That's done by delegating to the Channel (and reading an atomic pointer, but that one is
//   primitive op).
// * A bit of juggling with atomic and raw pointers. In effect, that is just late lazy
//   initialization, the Slot is in line with Option would be, except that it is set atomically
//   during the init. Lifetime is ensured by not dropping until the Drop of the whole slot and that
//   is checked by taking `&mut self`.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use libc::{c_int, siginfo_t};

use super::sealed::Exfiltrator;
use crate::low_level::channel::Channel;
pub use crate::low_level::siginfo::{Origin, Process};

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct Slot(AtomicPtr<Channel<Origin>>);

impl Drop for Slot {
    fn drop(&mut self) {
        let ptr = self.0.swap(ptr::null_mut(), Ordering::Acquire);
        if !ptr.is_null() {
            unsafe { drop(Box::from_raw(ptr)) }
        }
    }
}

/// The [`Exfiltrator`][crate::iterator::exfiltrator::Exfiltrator] that produces [`Origin`] of
/// signals.
///
/// # Examples
///
/// ```rust
/// # use signal_hook::consts::SIGUSR1;
/// # use signal_hook::iterator::SignalsInfo;
/// # use signal_hook::iterator::exfiltrator::WithOrigin;
/// #
/// # fn main() -> Result<(), std::io::Error> {
/// // Subscribe to SIGUSR1, with information about the process.
/// let mut signals = SignalsInfo::<WithOrigin>::new(&[SIGUSR1])?;
///
/// // Send a signal to ourselves.
/// let my_pid = unsafe { libc::getpid() };
/// unsafe { libc::kill(my_pid, SIGUSR1) };
///
/// // Grab the signal and look into the details.
/// let received = signals.wait().next().unwrap();
///
/// assert_eq!(SIGUSR1, received.signal);
/// assert_eq!(my_pid, received.process.unwrap().pid);
/// # Ok(()) }
/// ```
#[derive(Copy, Clone, Debug, Default)]
pub struct WithOrigin;

unsafe impl Exfiltrator for WithOrigin {
    type Storage = Slot;
    type Output = Origin;
    fn supports_signal(&self, _: c_int) -> bool {
        true
    }

    fn store(&self, slot: &Self::Storage, _: c_int, info: &siginfo_t) {
        let origin = unsafe { Origin::extract(info) };
        // Condition just not to crash if someone forgot to call init.
        //
        // Lifetime is from init to our own drop, and drop needs &mut self.
        if let Some(slot) = unsafe { slot.0.load(Ordering::Acquire).as_ref() } {
            slot.send(origin);
        }
    }

    fn load(&self, slot: &Self::Storage, _: c_int) -> Option<Origin> {
        let slot = unsafe { slot.0.load(Ordering::Acquire).as_ref() };
        // Condition just not to crash if someone forgot to call init.
        slot.and_then(|s| s.recv())
    }

    fn init(&self, slot: &Self::Storage, _: c_int) {
        let new = Box::new(Channel::default());
        let old = slot.0.swap(Box::into_raw(new), Ordering::Release);
        // We leak the pointer on purpose here. This is invalid state anyway and must not happen,
        // but if it still does, we can't drop that while some other thread might still be having
        // the raw pointer.
        assert!(old.is_null(), "Init called multiple times");
    }
}
