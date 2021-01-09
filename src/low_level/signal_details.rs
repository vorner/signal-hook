//! Providing auxiliary information for signals.

use libc::c_int;

use crate::consts::signal::*;

struct Details {
    signal: c_int,
    name: &'static str,
}

macro_rules! s {
    ($name: expr) => {
        Details {
            signal: $name,
            name: stringify!($name),
        }
    };
}

#[cfg(not(windows))]
const DETAILS: &[Details] = &[
    s!(SIGABRT),
    s!(SIGALRM),
    s!(SIGBUS),
    s!(SIGCHLD),
    s!(SIGCONT),
    s!(SIGFPE),
    s!(SIGHUP),
    s!(SIGILL),
    s!(SIGINT),
    s!(SIGIO),
    s!(SIGKILL),
    s!(SIGPIPE),
    s!(SIGPROF),
    s!(SIGQUIT),
    s!(SIGSEGV),
    s!(SIGSTOP),
    s!(SIGSYS),
    s!(SIGTERM),
    s!(SIGTRAP),
    s!(SIGTSTP),
    s!(SIGTTIN),
    s!(SIGTTOU),
    s!(SIGURG),
    s!(SIGUSR1),
    s!(SIGUSR2),
    s!(SIGVTALRM),
    s!(SIGWINCH),
    s!(SIGXCPU),
    s!(SIGXFSZ),
];

#[cfg(windows)]
const DETAILS: &[Details] = &[
    s!(SIGABRT),
    s!(SIGFPE),
    s!(SIGILL),
    s!(SIGINT),
    s!(SIGSEGV),
    s!(SIGTERM),
];

/// Provides a human-readable name of a signal.
///
/// Note that the name does not have to be known (in case it is some less common, or non-standard
/// signal).
///
/// # Examples
///
/// ```
/// # use signal_hook::low_level::signal_name;
/// assert_eq!("SIGKILL", signal_name(9).unwrap());
/// assert!(signal_name(142).is_none());
/// ```
pub fn signal_name(signal: c_int) -> Option<&'static str> {
    DETAILS.iter().find(|d| d.signal == signal).map(|d| d.name)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn existing() {
        assert_eq!("SIGTERM", signal_name(SIGTERM).unwrap());
    }

    #[test]
    fn unknown() {
        assert!(signal_name(128).is_none());
    }
}
