//! Low-level internals of [`signal-hook`](https://docs.rs/signal-hook).
//!
//! This crate contains some internal APIs, split off to a separate crate for technical reasons. Do
//! not use directly. There are no stability guarantees, no documentation and you should use
//! `signal-hook` directly.

#[doc(hidden)]
pub mod internal {
    use libc::{pid_t, siginfo_t, uid_t};

    // Careful: make sure the signature and the constants match the C source
    extern "C" {
        fn sighook_signal_cause(info: &siginfo_t) -> Cause;
        fn sighook_signal_pid(info: &siginfo_t) -> pid_t;
        fn sighook_signal_uid(info: &siginfo_t) -> uid_t;
    }

    // Warning: must be in sync with the C code
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    #[non_exhaustive]
    #[repr(u8)]
    pub enum Cause {
        Unknown = 0,
        Kernel = 1,
        User = 2,
        TKill = 3,
        Queue = 4,
        MesgQ = 5,
        Exited = 6,
        Killed = 7,
        Dumped = 8,
        Trapped = 9,
        Stopped = 10,
        Continued = 11,
    }

    impl Cause {
        // The MacOs doesn't use the SI_* constants and leaves si_code at 0. But it doesn't use an
        // union, it has a good-behaved struct with fields and therefore we *can* read the values,
        // even though they'd contain nonsense (zeroes). We wipe that out later.
        #[cfg(target_os = "macos")]
        fn has_process(self) -> bool {
            true
        }

        #[cfg(not(target_os = "macos"))]
        fn has_process(self) -> bool {
            use Cause::*;
            match self {
                Unknown | Kernel => false,
                User | TKill | Queue | MesgQ | Exited | Killed | Dumped | Trapped | Stopped
                | Continued => true,
            }
        }
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    #[non_exhaustive]
    pub struct Process {
        pub pid: pid_t,
        pub uid: uid_t,
    }

    impl Process {
        /**
         * Extract the process information.
         *
         * # Safety
         *
         * The `info` must have a `si_code` corresponding to some situation that has the `si_pid`
         * and `si_uid` filled in.
         */
        unsafe fn extract(info: &siginfo_t) -> Self {
            Self {
                pid: sighook_signal_pid(info),
                uid: sighook_signal_uid(info),
            }
        }

        pub const fn to_u64(self) -> u64 {
            let pid = self.pid as u32; // With overflow for negative ones
            let uid = self.uid as u32;
            ((pid as u64) << 32) | (uid as u64)
        }

        pub const fn from_u64(encoded: u64) -> Self {
            let pid = ((encoded >> 32) as u32) as _;
            let uid = (encoded as u32) as _;
            Self { pid, uid }
        }

        pub const EMPTY: Self = Self { pid: -1, uid: 0 };

        pub const NO_PROCESS: Self = Self { pid: -1, uid: 1 };
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    #[non_exhaustive]
    pub struct SigInfo {
        pub cause: Cause,
        pub process: Option<Process>,
    }

    impl SigInfo {
        // Note: shall be async-signal-safe
        pub fn extract(info: &siginfo_t) -> Self {
            let cause = unsafe { sighook_signal_cause(info) };
            let process = if cause.has_process() {
                let process = unsafe { Process::extract(info) };
                // On macos we don't have the si_code to go by, but we can go by the values being
                // empty there.
                if cfg!(target_os = "macos") && process.pid == 0 && process.uid == 0 {
                    None
                } else {
                    Some(process)
                }
            } else {
                None
            };
            Self { cause, process }
        }
    }
}
