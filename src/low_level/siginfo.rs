//! Extracting more information from the C [`siginfo_t`] structure.
//!
//! See [`Origin`].

use libc::{c_int, pid_t, siginfo_t, uid_t};
use signal_hook_sys::internal::{Cause as ICause, SigInfo};

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
/// This is produced by the [`WithOrigin`] exfiltrator (or can be [extracted][Origin::extract] from
/// `siginfo_t` by hand).
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
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

impl Origin {
    /// Extracts the Origin from a raw `siginfo_t` structure.
    ///
    /// This function is async-signal-safe, can be called inside a signal handler.
    ///
    /// # Safety
    ///
    /// On systems where the structure is backed by an union on the C side, this requires the
    /// `si_code` and `si_signo` fields must be set properly according to what fields are
    /// available.
    ///
    /// The value passed by kernel satisfies this, care must be taken only when constructed
    /// manually.
    pub unsafe fn extract(info: &siginfo_t) -> Self {
        let signal = info.si_signo;
        let extracted = SigInfo::extract(info);
        let process = extracted.process.map(|p| Process {
            pid: p.pid,
            uid: p.uid,
        });
        Origin {
            cause: extracted.cause.into(),
            signal,
            process,
        }
    }
}
