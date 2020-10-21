//! Module with the self-pipe pattern.
//!
//! One of the common patterns around signals is to have a pipe with both ends in the same program.
//! Whenever there's a signal, the signal handler writes one byte of garbage data to the write end,
//! unless the pipe's already full. The application then can handle the read end.
//!
//! This has two advantages. First, the real signal action moves outside of the signal handler
//! where there are a lot less restrictions. Second, it fits nicely in all kinds of asynchronous
//! loops and has less chance of race conditions.
//!
//! This module offers premade functions for the write end (and doesn't insist that it must be a
//! pipe ‒ anything that can be written to is fine ‒ sockets too, therefore `UnixStream::pair` is a
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
//! extern crate signal_hook;
//!
//! use std::io::{Error, Read};
//! use std::os::unix::net::UnixStream;
//!
//! fn main() -> Result<(), Error> {
//!     let (mut read, write) = UnixStream::pair()?;
//!     signal_hook::pipe::register(signal_hook::SIGUSR1, write)?;
//!     // This will write into the pipe write end through the signal handler
//!     unsafe { libc::raise(signal_hook::SIGUSR1) };
//!     let mut buff = [0];
//!     read.read_exact(&mut buff)?;
//!     println!("Happily terminating");
//!     Ok(())
//! }

use std::io::{Error, ErrorKind};
use std::os::unix::io::{AsRawFd, IntoRawFd, RawFd};

use libc::{self, c_int, siginfo_t, size_t};

use crate::SigId;

/// nfx: Document it!
pub trait ExtractAndBuffer: Clone + Copy {
    /// nfx: Document it!
    fn new(source: &siginfo_t) -> Self;
    /// nfx: Document it!
    fn to_buffer(&self, buffer: &mut [u8]) -> u8 {
        let self_size = std::mem::size_of::<Self>();
        if self_size < buffer.len() {
            let src = self as *const _ as *const u8;
            let dst = &mut buffer[..] as *mut _ as *mut u8;
            unsafe { std::ptr::copy_nonoverlapping(src, dst, self_size) };
            self_size as u8
        }
        else {
            0
        }
    }
    /// nfx: Document it!
    unsafe fn from_buffer(&mut self, buffer: &[u8]) -> bool {
        let self_size = std::mem::size_of::<Self>();
        if self_size == buffer.len() {

            let src = &buffer[..] as *const _ as *const u8;
            let dst = self as *mut _ as *mut u8;
            std::ptr::copy_nonoverlapping(src, dst, self_size);
            true
        }
        else {
            false
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) enum WakeMethod {
    Send,
    Write,
}

struct WakeFd {
    fd: RawFd,
    method: WakeMethod,
}

impl WakeFd {
    /// Sets close on exec and nonblock on the inner file descriptor.
    fn set_flags(&self) -> Result<(), Error> {
        unsafe {
            let flags = libc::fcntl(self.as_raw_fd(), libc::F_GETFL, 0);
            if flags == -1 {
                return Err(Error::last_os_error());
            }
            let flags = flags | libc::O_NONBLOCK | libc::O_CLOEXEC;
            if libc::fcntl(self.as_raw_fd(), libc::F_SETFL, flags) == -1 {
                return Err(Error::last_os_error());
            }
        }
        Ok(())
    }
    fn wake(&self, buffer: Option<&[u8]>) {
        wake(self.fd, self.method, buffer);
    }
}

impl AsRawFd for WakeFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for WakeFd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

pub(crate) fn wake(pipe: RawFd, method: WakeMethod, buffer: Option<&[u8]>) {
    unsafe {
        // This writes some data into the pipe.
        //
        // There are two tricks:
        // * First, the crazy cast. The first part turns reference into pointer. The second part
        //   turns pointer to u8 into a pointer to void, which is what write requires.
        // * Second, we ignore errors, on purpose. We don't have any means to handling them. The
        //   two conceivable errors are EBADFD, if someone passes a non-existent file descriptor or
        //   if it is closed. The second is EAGAIN, in which case the pipe is full ‒ there were
        //   many signals, but the reader didn't have time to read the data yet. It'll still get
        //   woken up, so not fitting another letter in it is fine.

        let (data, amount) = if let Some(buffer) = buffer {
            (buffer as *const _ as *const _, buffer.len())
        }
        else {
            (b"X" as *const _ as *const _, 1)
        };

        let written = match method {
            WakeMethod::Write => libc::write(pipe, data, amount),
            WakeMethod::Send => libc::send(pipe, data, amount, libc::MSG_DONTWAIT),

        };
        // There are three possibilites for libc:write:
        // 1. All good.  In which case written == amount (after casting).
        // 2. Some written.  In which case written < amount (after casting; written is not -1).
        //    This possibility is a hard failure.
        // 3. Error.  In this case written is -1.  Based on
        //    https://www.man7.org/linux/man-pages/man2/write.2.html it seems safe to assume the
        //    failure is atomic (nothing written).  In addition, if this were not true there would
        //    be no way to know how many bytes were actually written which would it impossible to
        //    recover from any error.
        //    unrecoverable.

        if written as size_t != amount && amount > 1 && written > 0 {
            // unsafe
            {
                const MSG: &[u8] =
                    b"Write method not atomic in pipe::wake. Aborting";
                libc::write(2, MSG.as_ptr() as *const _, MSG.len());
                libc::abort();
            }
        }
    }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// In this case, the pipe is taken as the `RawFd`. It'll be closed on deregistration. Effectively,
/// the function takes ownership of the file descriptor. This includes feeling free to set arbitrary
/// flags on it, including file status flags (that are shared across file descriptors created by
/// `dup`).
///
/// Note that passing the wrong file descriptor won't cause UB, but can still lead to severe bugs ‒
/// like data corruptions in files. Prefer using [`register`] if possible.
///
/// Also, it is perfectly legal for multiple writes to be collated together (if not consumed) and
/// to generate spurious wakeups (but will not generate spurious *bytes* in the pipe).
///
/// # Internal details
///
/// Internally, it *currently* does following. Note that this is *not* part of the stability
/// guarantees and may change if necessary.
///
/// * If the file descriptor can be used with [`send`][libc::send], it'll be used together with
///   [`MSG_DONTWAIT`][libc::MSG_DONTWAIT]. This is tested by sending `0` bytes of data (depending
///   on the socket type, this might wake the read end with an empty message).
/// * If it is not possible, the [`O_NONBLOCK`][libc::O_NONBLOCK] will be set on the file
///   descriptor and [`write`][libc::write] will be used instead.
pub fn register_raw(signal: c_int, pipe: RawFd) -> Result<SigId, Error> {
    let fd = determine_wake_method(pipe)?;
    unsafe { crate::register(signal, move ||{
        fd.wake(None);
    }) }
}

/// Registers a write to a self-pipe whenever there's the signal.
///
/// The ownership of pipe is taken and will be closed whenever the created action is unregistered.
///
/// Note that if you want to register the same pipe for multiple signals, there's `try_clone`
/// method on many unix socket primitives.
///
/// See [`register_raw`] for further details.
pub fn register<P>(signal: c_int, pipe: P) -> Result<SigId, Error>
where
    P: IntoRawFd + 'static,
{
    register_raw(signal, pipe.into_raw_fd())
}

/// The maximum number of bytes (maximum struct size) supported by
/// register_struct / register_struct_raw.  If the maximum size is exceeded a single null is
/// written indicating a zero-sized struct.  Ideally, the size would be checked at compile-time but
/// does not appear to be supported at the time this was written.
pub const STRUCT_SIZE_MAX: usize = 63;

/// nfx: Document it!
pub fn register_struct_raw<T>(signal: c_int, pipe: RawFd) -> Result<SigId, Error>
where
    T: ExtractAndBuffer,
{
    let fd = determine_wake_method(pipe)?;
    unsafe { crate::register_sigaction(signal, move |source|{
        let s = T::new(source);
        let mut buffer: [u8; STRUCT_SIZE_MAX+1] = [0; STRUCT_SIZE_MAX+1];
        let size = s.to_buffer(&mut buffer[1..]);
        buffer[0] = size;
        let size = size as usize;
        fd.wake(Some(&buffer[0..size+1]));
    }) }
}

/// Registers writing customized data to a self-pipe whenever there's the signal.
///
/// # Examples
///
/// This example mimics the behaviour of [`register`].  Instead of writing an ASCII X (b"X") it
/// writes a null (0u8) but is otherwise indistinguishable.
///
/// ```
/// use std::io::Read;
/// use std::os::unix::net::UnixStream;
///
/// use libc::{siginfo_t};
/// use signal_hook::pipe::{ExtractAndBuffer, register_struct};
///
/// #[derive(Clone, Copy)]
/// struct JustNull {}
///
/// impl ExtractAndBuffer for JustNull {
///     fn new(_source: &siginfo_t) -> Self {
///         Self {}
///     }
///     fn to_buffer(&self, _fill_this: &mut [u8]) -> u8 {
///         0
///     }
/// }
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let (mut read, write) = UnixStream::pair()?;
///     println!();
///     // Use JustNull to write to our pipe when SIGUSR1 is received
///     let id = register_struct::<JustNull, _>(signal_hook::SIGUSR1, write)?;
///     println!("id = {:?}", id);
///     // Send ourselves SIGUSR1 in bit
///     std::thread::spawn(move || {
///         std::thread::sleep(std::time::Duration::from_millis(250));
///         unsafe { assert_eq!(0, libc::raise(libc::SIGUSR1)) };
///     });
///     let mut x = [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255];
///     // Wait for the signal
///     let c = read.read(&mut x)?;
///     println!();
///     println!("c = {:?}", c);
///     println!("x = {:?}", x);
///     println!();
///     assert!(c == 1 && x[0] == 0 && x[1] == 255);
///     Ok(())
/// }
/// ```
///
/// This example writes the process ID (pid) and user ID (uid) of the sending application then reads those in a threaded signal handler.
///
/// ```
/// use std::io::Read;
/// use std::os::unix::net::UnixStream;
/// use std::thread::{JoinHandle, spawn};
/// use std::time::Duration;
///
/// use libc::{pid_t, siginfo_t, uid_t};
/// use signal_hook::pipe::{ExtractAndBuffer, register_struct, STRUCT_SIZE_MAX};
///
/// #[derive(Clone, Copy, Debug)]
/// struct Killer {
///     pid: pid_t,
///     uid: uid_t,
/// }
///
/// impl ExtractAndBuffer for Killer {
///     fn new(source: &siginfo_t) -> Self {
///         Self {
///             pid: unsafe { source.si_pid() },
///             uid: unsafe { source.si_uid() },
///         }
///     }
/// }
///
/// impl Default for Killer {
///     fn default() -> Self {
///         Self {
///             pid: pid_t::MAX,
///             uid: uid_t::MAX,
///         }
///     }
/// }
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     println!();
///     let (mut read, write) = UnixStream::pair()?;
///
///     // Use Killer to write pid and uid to our pipe when SIGUSR1 is received
///     let id = register_struct::<Killer, _>(signal_hook::SIGUSR1, write)?;
///     println!("id = {:?}", id);
///
///     // Handle the signal in a thread
///     let wait_1: JoinHandle<Result<(), std::io::Error>> = spawn(move || {
///         let mut size = [0];
///         // Block here until the signal arrives
///         read.read_exact(&mut size)?;
///         println!();
///         println!("si = {:?}", size[0]);
///         // How many bytes were written?
///         let expected = size[0] as usize;
///         if expected > 0 {
///             // Try reading all those bytes
///             let mut buffer = [0; STRUCT_SIZE_MAX];
///             read.set_read_timeout(Some(Duration::from_millis(100)))?;
///             let actual = read.read(&mut buffer[0..expected])?;
///             println!("ac = {:?}", actual);
///             println!("bu = {:?}", buffer);
///             println!();
///             // Try stuffing those bytes into our struct
///             let mut killer = Killer::default();
///             if unsafe { killer.from_buffer(&buffer[0..actual]) } {
///                 println!("killer = {:#?}", killer);
///             }
///             else {
///                 println!("killer.from_buffer failed!");
///             }
///         }
///         else {
///             println!("Zero-sized struct sent but Killer was expected!");
///         }
///         Ok(())
///     });
///
///     // Send ourselves SIGUSR1 in bit
///     let wait_2 = spawn(move || {
///         std::thread::sleep(Duration::from_millis(250));
///         unsafe { assert_eq!(0, libc::raise(libc::SIGUSR1)) };
///     });
///
///     // Wait here
///     wait_1.join().unwrap()?;
///     wait_2.join().unwrap();
///
///     println!();
///     Ok(())
/// }
/// ```
///
/// See [`register`] for details regarding the ownership of the pipe.
pub fn register_struct<T, P>(signal: c_int, pipe: P) -> Result<SigId, Error>
where
    T: ExtractAndBuffer,
    P: IntoRawFd + 'static,
{
    register_struct_raw::<T>(signal, pipe.into_raw_fd())
}

fn determine_wake_method(pipe: RawFd) -> Result<WakeFd, Error> {
    let res = unsafe { libc::send(pipe, &[] as *const _, 0, libc::MSG_DONTWAIT) };
    Ok(match (res, Error::last_os_error().kind()) {
        (0, _) | (-1, ErrorKind::WouldBlock) => WakeFd {
            fd: pipe,
            method: WakeMethod::Send,
        },
        _ => {
            let fd = WakeFd {
                fd: pipe,
                method: WakeMethod::Write,
            };
            fd.set_flags()?;
            fd
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::unix::net::{UnixDatagram, UnixStream};

    use super::*;

    // Note: multiple tests share the SIGUSR1 signal. This is fine, we only need to know the signal
    // arrives. It's OK to arrive multiple times, from multiple tests.
    fn wakeup() {
        unsafe { assert_eq!(0, libc::raise(libc::SIGUSR1)) }
    }

    #[test]
    fn register_with_socket() -> Result<(), Error> {
        let (mut read, write) = UnixStream::pair()?;
        register(libc::SIGUSR1, write)?;
        wakeup();
        let mut buff = [0; 1];
        read.read_exact(&mut buff)?;
        assert_eq!(b"X", &buff);
        Ok(())
    }

    #[test]
    fn register_dgram_socket() -> Result<(), Error> {
        let (read, write) = UnixDatagram::pair()?;
        register(libc::SIGUSR1, write)?;
        wakeup();
        let mut buff = [0; 1];
        // The attempt to detect if it is socket can generate an empty message. Therefore, do a few
        // retries.
        for _ in 0..3 {
            let len = read.recv(&mut buff)?;
            if len == 1 && &buff == b"X" {
                return Ok(());
            }
        }
        panic!("Haven't received the right data");
    }

    #[test]
    fn register_with_pipe() -> Result<(), Error> {
        let mut fds = [0; 2];
        unsafe { assert_eq!(0, libc::pipe(fds.as_mut_ptr())) };
        register_raw(libc::SIGUSR1, fds[1])?;
        wakeup();
        let mut buff = [0; 1];
        unsafe { assert_eq!(1, libc::read(fds[0], buff.as_mut_ptr() as *mut _, 1)) }
        assert_eq!(b"X", &buff);
        Ok(())
    }
}
