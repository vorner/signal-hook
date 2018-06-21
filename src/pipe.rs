//! Module with the self-pipe pattern.
//!
//! One of the common patterns around signals is to have a pipe with both ends in the same program.
//! Whenever there's a signal, the signal handler writes to the write end. The application then can
//! handle the read end.
//!
//! This has two advantages. First, the real signal handling moves outside of the signal handler
//! where there are a lot less restrictions. Second, it fits nicely in all kinds of asynchronous
//! loops, has less chance of race conditions.
//!
//! This module offers premade functions for the write end (and doesn't insist that it must be a
//! pipe ‒ anything that can be `sent` to is fine ‒ sockets too, therefore `UnixStream::pair` is a
//! good candidate).
//!
//! If it looks too low-level for your needs, the [`iterator`](iterator/) module contains some
//! higher-lever interface that also uses a self-pipe pattern under the hood.
//!
//! TODO: Examples and correct order, note about spurious wakeups

use std::io::Error;
use std::os::unix::io::{AsRawFd, RawFd};

use libc::{self, c_int};

use SigId;

pub(crate) fn wake(pipe: RawFd) {
    unsafe {
        // This sends some data into the pipe.
        //
        // There are two tricks:
        // * First, the crazy cast. The first part turns reference into pointer. The second part
        //   turns pointer to u8 into a pointer to void, which is what send requires.
        // * Second, we ignore errors, on purpose. We don't have any means to handling them. The
        //   two conceivable errors are EBADFD, if someone passes a non-existent file descriptor or
        //   if it is closed. The second is EAGAIN, in which case the pipe is full ‒ there were
        //   many signals, but the reader didn't have time to read the data yet. It'll still get
        //   woken up, so not fitting another letter in it is fine.
        libc::send(pipe, b"X" as *const _ as *const _, 1, libc::MSG_DONTWAIT);
    }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// In this case, the pipe is taken as the `RawFd`. It is still the caller's responsibility to
/// close it.
pub fn self_pipe_raw(signal: c_int, pipe: RawFd) -> Result<SigId, Error> {
    unsafe { ::register(signal, move || wake(pipe)) }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// The ownership of pipe is taken and will be closed whenever the created action is unregistered.
///
/// Note that if you want to register the same pipe for multiple signals, there's `try_clone`
/// method on many unix socket primitives.
pub fn self_pipe<P>(signal: c_int, pipe: P) -> Result<SigId, Error>
where
    P: AsRawFd + Send + Sync + 'static,
{
    let action = move || wake(pipe.as_raw_fd());
    unsafe { ::register(signal, action) }
}
