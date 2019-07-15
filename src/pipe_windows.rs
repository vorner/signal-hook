//! Module with the self-pipe pattern.
//!
//! One of the common patterns around signals is to have a pipe with both ends in the same program.
//! Whenever there's a signal, the signal handler writes to the write end. The application then can
//! handle the read end.
//!
//! This has two advantages. First, the real signal action moves outside of the signal handler
//! where there are a lot less restrictions. Second, it fits nicely in all kinds of asynchronous
//! loops and has less chance of race conditions.
//!
//! This module offers premade functions for the write end (and doesn't insist that it must be a
//! pipe ‒ anything that can be `sent` to is fine ‒ sockets too, therefore `UnixStream::pair` is a
//! good candidate).
//!
//! If you want to integrate with some asynchronous library, plugging streams from `mio-uds` or
//! `tokio-uds` libraries should work.
//!
//! If it looks too low-level for your needs, the [`iterator`](../iterator/) module contains some
//! higher-lever interface that also uses a self-pipe pattern under the hood.
//!
//! # Correct order of handling
//!
//! A care needs to be taken to avoid race conditions, especially when handling the same signal in
//! a loop. Specifically, another signal might come when the action for the previous signal is
//! being taken. The correct order is first to clear the content of the pipe (read some/all data
//! from it) and then take the action. This way a spurious wakeup can happen (the pipe could wake
//! up even when no signal came after the signal was taken, because ‒ it arrived between cleaning
//! the pipe and taking the action). Note that some OS primitives (eg. `select`) suffer from
//! spurious wakeups themselves (they can claim a FD is readable when it is not true) and blocking
//! `read` might return prematurely (with eg. `EINTR`).
//!
//! The reverse order of first taking the action and then clearing the pipe might lose signals,
//! which is usually worse.
//!
//! This is not a problem with blocking on reading from the pipe (because both the blocking and
//! cleaning is the same action), but in case of asynchronous handling it matters.
//!
//! If you want to combine setting some flags with a self-pipe pattern, the flag needs to be set
//! first, then the pipe written. On the read end, first the pipe needs to be cleaned, then the
//! flag and then the action taken. This is what the [`Signals`](../iterator/struct.Signals.html)
//! structure does internally.
//!
//! # Write collating
//!
//! While unlikely if handled correctly, it is possible the write end is full when a signal comes.
//! In such case the signal handler simply does nothing. If the write end is full, the read end is
//! readable and therefore will wake up. On the other hand, blocking in the signal handler would
//! definitely be a bad idea.
//!
//! However, this also means the number of bytes read from the end might be lower than the number
//! of signals that arrived. This should not generally be a problem, since the OS already collates
//! signals of the same kind together.
//!
//! # Examples
//!
//! This example waits for at last one `SIGUSR1` signal to come before continuing (and
//! terminating). It sends the signal to itself, so it correctly terminates.
//!
//! ```rust
//! extern crate libc;
//! extern crate winapi;
//! extern crate signal_hook;
//!
//! use std::fs::File;
//! use std::io::{Error, Read};
//! use std::ptr;
//! use std::os::windows::prelude::*;
//! use winapi::um::namedpipeapi::CreatePipe;
//! use winapi::um::winnt::HANDLE;
//!
//! fn main() -> Result<(), Error> {
//!     let (mut read, write) = {
//!         let mut read: HANDLE = ptr::null_mut();
//!         let mut write: HANDLE = ptr::null_mut();
//!         let success = unsafe { CreatePipe(&mut read, &mut write, ptr::null_mut(), 1024) } != 0;
//!         assert!(success);
//!         let read = unsafe { File::from_raw_handle(read as RawHandle) };
//!         let write = unsafe { File::from_raw_handle(write as RawHandle) };
//!         (read, write)
//!     };
//!     signal_hook::pipe_windows::register_handle(signal_hook::SIGINT, write)?;
//! # fn do_something_that_may_be_interrupted_by_ctrl_c() {}
//!     do_something_that_may_be_interrupted_by_ctrl_c();
//!     let mut buff = [0];
//! # if false {
//!     read.read_exact(&mut buff)?;
//! # }
//!     println!("Happily terminating");
//!     Ok(())
//! }
//! ```

use std::io::Error;
use std::os::windows::prelude::*;
use std::ptr;

use libc::c_int;
use winapi::um::fileapi::WriteFile;
use winapi::um::winnt::HANDLE;
use winapi::um::winsock2::{send, SOCKET};

use SigId;

pub(crate) fn wake_handle(pipe: RawHandle) {
    unsafe {
        let mut num_written = 0;
        let _success = WriteFile(
            pipe as HANDLE,
            b"X" as *const _ as *const _,
            1,
            &mut num_written,
            ptr::null_mut(),
        );
    }
}

pub(crate) fn wake_socket(pipe: RawSocket) {
    unsafe {
        send(pipe as SOCKET, b"X" as *const _ as *const _, 1, 0);
    }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// In this case, the pipe is taken as the `RawHandle`. It is still the caller's responsibility to
/// close it.
pub fn register_handle_raw(signal: c_int, pipe: RawHandle) -> Result<SigId, Error> {
    struct SendHandle(RawHandle);
    unsafe impl Send for SendHandle {}
    unsafe impl Sync for SendHandle {}
    let pipe = SendHandle(pipe);
    unsafe { ::register(signal, move || wake_handle(pipe.0)) }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// In this case, the pipe is taken as the `RawSocket`. It is still the caller's responsibility to
/// close it.
pub fn register_socket_raw(signal: c_int, pipe: RawSocket) -> Result<SigId, Error> {
    struct SendSocket(RawSocket);
    unsafe impl Send for SendSocket {}
    unsafe impl Sync for SendSocket {}
    let pipe = SendSocket(pipe);
    unsafe { ::register(signal, move || wake_socket(pipe.0)) }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// The ownership of pipe is taken and will be closed whenever the created action is unregistered.
///
/// Note that if you want to register the same pipe for multiple signals, there's `try_clone`
/// method on many unix socket primitives.
pub fn register_handle<P>(signal: c_int, pipe: P) -> Result<SigId, Error>
where
    P: AsRawHandle + Send + Sync + 'static,
{
    let action = move || wake_handle(pipe.as_raw_handle());
    unsafe { ::register(signal, action) }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// The ownership of pipe is taken and will be closed whenever the created action is unregistered.
///
/// Note that if you want to register the same pipe for multiple signals, there's `try_clone`
/// method on many unix socket primitives.
pub fn register_socket<P>(signal: c_int, pipe: P) -> Result<SigId, Error>
where
    P: AsRawSocket + Send + Sync + 'static,
{
    let action = move || wake_socket(pipe.as_raw_socket());
    unsafe { ::register(signal, action) }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::io::prelude::*;
    use std::ptr;
    use winapi::um::namedpipeapi::CreatePipe;
    use winapi::um::winnt::HANDLE;

    #[test]
    fn pipe_notify() -> Result<(), ::std::io::Error> {
        let (mut read, write) = {
            let mut read: HANDLE = ptr::null_mut();
            let mut write: HANDLE = ptr::null_mut();
            let success = unsafe { CreatePipe(&mut read, &mut write, ptr::null_mut(), 1024) } != 0;
            assert!(success);
            let read = unsafe { File::from_raw_handle(read as RawHandle) };
            let write = unsafe { File::from_raw_handle(write as RawHandle) };
            (read, write)
        };
        register_handle(::SIGUSR1, write)?;
        // This will write into the pipe write end through the signal handler
        unsafe { ::__emulate_kill(libc::getpid(), ::SIGUSR1) };
        let mut buff = [0];
        read.read_exact(&mut buff)?;
        println!("Happily terminating");
        Ok(())
    }
}
