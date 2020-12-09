//! An exfiltrator providing the process that caused the signal.
//!
//! The [`WithOrigin`] is an [`Exfiltrator`][crate::iterator::exfiltrator::Exfiltrator] that
//! provides the information about sending process in addition to the signal number, through the
//! [`Origin`] type.
//!
//! See the [`WithOrigin`] example.

use std::sync::atomic::{AtomicU64, Ordering};

use libc::{c_int, pid_t, siginfo_t, uid_t};
use signal_hook_sys::internal::{Process as IProcess, SigInfo};

use super::sealed::Exfiltrator;

/// Information about process, as presented in the signal metadata.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Process {
    /// The process ID.
    pub pid: pid_t,

    /// The user owning the process.
    pub uid: uid_t,
}

impl From<IProcess> for Process {
    fn from(p: IProcess) -> Self {
        Self {
            pid: p.pid,
            uid: p.uid,
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
    // TODO: Figure out a better encoding somehow and expose other info, including the Cause
}

#[doc(hidden)]
#[derive(Debug)]
pub struct OriginStorage(AtomicU64);

impl Default for OriginStorage {
    fn default() -> Self {
        Self(AtomicU64::new(IProcess::EMPTY.to_u64()))
    }
}

/// The [`Exfiltrator`][crate::iterator::exfiltrator::Exfiltrator] that produces [`Origin`] of
/// signals.
///
/// # Examples
///
/// ```rust
/// # use signal_hook::SIGUSR1;
/// # use signal_hook::iterator::SignalsInfo;
/// # use signal_hook::iterator::exfiltrator::origin::WithOrigin;
/// #
/// # fn main() -> Result<(), std::io::Error> {
/// // Subscribe to SIGUSR1, with information about the process.
/// let signals = SignalsInfo::<WithOrigin>::new(&[SIGUSR1])?;
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
    type Storage = OriginStorage;
    type Output = Origin;
    fn supports_signal(&self, _: c_int) -> bool {
        true
    }

    fn store(&self, slot: &OriginStorage, _: c_int, info: &siginfo_t) {
        let value = SigInfo::extract(info)
            .process
            .unwrap_or(IProcess::NO_PROCESS)
            .to_u64();
        slot.0.store(value, Ordering::SeqCst);
    }

    fn load(&self, slot: &OriginStorage, signal: c_int) -> Option<Origin> {
        let value = slot.0.swap(IProcess::EMPTY.to_u64(), Ordering::SeqCst);
        match IProcess::from_u64(value) {
            IProcess::EMPTY => None,
            IProcess::NO_PROCESS => Some(Origin {
                signal,
                process: None,
            }),
            process => Some(Origin {
                signal,
                process: Some(process.into()),
            }),
        }
    }
}
