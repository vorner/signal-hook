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

use libc::{c_int, pid_t, siginfo_t, uid_t};
use signal_hook_sys::internal::{Cause as ICause, SigInfo};

use super::sealed::Exfiltrator;
use crate::channel::Channel;

/// Information about process, as presented in the signal metadata.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Process {
    /// The process ID.
    pub pid: pid_t,

    /// The user owning the process.
    pub uid: uid_t,
}

/// The means by which a signal was sent by other process.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Sent {
    /// The `kill` call.
    User,

    /// The `tkill` call.
    ///
    /// This is likely linux specific.
    TKill,

    /// `sigqueue`.
    Queue,

    /// `mq_notify`.
    MesgQ,
}

/// A child changed its state.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Chld {
    /// The child exited normally.
    Exited,

    /// It got killed by a signal.
    Killed,

    /// It got killed by a signal and dumped core.
    Dumped,

    /// The child was trapped by a `SIGTRAP` signal.
    Trapped,

    /// The child got stopped.
    Stopped,

    /// The child continued (after being stopped).
    Continued,
}

/// What caused a signal.
///
/// This is a best-effort (and possibly incomplete) representation of the C `siginfo_t::si_code`.
/// It may differ between OSes and may be extended in future versions.
///
/// Note that this doesn't contain all the „fault“ signals (`SIGILL`, `SIGSEGV` and similar).
/// There's no reasonable way to use the exfiltrators with them, since the handler either needs to
/// terminate the process or somehow recover from the situation. Things based on exfiltrators do
/// neither, which would cause an UB and therefore these values just don't make sense.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Cause {
    /// The cause is unknown.
    ///
    /// Some systems don't fill this in. Some systems have values we don't understand. Some signals
    /// don't have specific reasons to come to being.
    Unknown,

    /// Sent by the kernel.
    ///
    /// This probably exists only on Linux.
    Kernel,

    /// The signal was sent by other process.
    Sent(Sent),

    /// A `SIGCHLD`, caused by a child process changing state.
    Chld(Chld),
}

impl From<ICause> for Cause {
    fn from(c: ICause) -> Cause {
        match c {
            ICause::Kernel => Cause::Kernel,
            ICause::User => Cause::Sent(Sent::User),
            ICause::TKill => Cause::Sent(Sent::TKill),
            ICause::Queue => Cause::Sent(Sent::Queue),
            ICause::MesgQ => Cause::Sent(Sent::MesgQ),
            ICause::Exited => Cause::Chld(Chld::Exited),
            ICause::Killed => Cause::Chld(Chld::Killed),
            ICause::Dumped => Cause::Chld(Chld::Dumped),
            ICause::Trapped => Cause::Chld(Chld::Trapped),
            ICause::Stopped => Cause::Chld(Chld::Stopped),
            ICause::Continued => Cause::Chld(Chld::Continued),
            // Unknown and possibly others if the underlying lib is updated
            _ => Cause::Unknown,
        }
    }
}

/// Information about a signal and its origin.
///
/// This is produced by the [`WithOrigin`] exfiltrator. See the example there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Origin {
    /// The signal that happened.
    pub signal: c_int,

    /// Information about the process that caused the signal.
    ///
    /// Note that not all signals are caused by a specific process or have the information
    /// available („fault“ signals like `SIGBUS` don't have, any signal may be sent by the kernel
    /// instead of a specific process).
    ///
    /// This is filled in whenever available. For most signals, this is the process that sent the
    /// signal (by `kill` or similar), for `SIGCHLD` it is the child that caused the signal.
    pub process: Option<Process>,

    /// How the signal happened.
    ///
    /// This is a best-effort value. In particular, some systems may have causes not known to this
    /// library. Some other systems (MacOS) does not fill the value in so there's no way to know.
    /// In all these cases, this will contain [`Cause::Unknown`].
    ///
    /// Some values are platform specific and not available on other systems.
    ///
    /// Future versions may enrich the enum by further values.
    pub cause: Cause,
}

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
/// # use signal_hook::iterator::exfiltrator::origin::WithOrigin;
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
/// let received = signals.forever().next().unwrap();
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

    fn store(&self, slot: &Self::Storage, signal: c_int, info: &siginfo_t) {
        let extracted = SigInfo::extract(info);
        let process = extracted.process.map(|p| Process {
            pid: p.pid,
            uid: p.uid,
        });
        let origin = Origin {
            cause: extracted.cause.into(),
            signal,
            process,
        };
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
