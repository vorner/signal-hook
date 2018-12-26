//! An iterator over incoming signals.
//!
//! This provides a higher abstraction over the signals, providing a structure
//! ([`Signals`](struct.Signals.html)) able to iterate over the incoming signals.
//!
//! In case the `tokio-support` feature is turned on, the [`Async`](struct.Async.html) is also
//! available, making it possible to integrate with the tokio runtime.
//!
//! # Examples
//!
//! ```rust
//! extern crate libc;
//! extern crate signal_hook;
//!
//! use std::io::Error;
//!
//! use signal_hook::iterator::Signals;
//!
//! fn main() -> Result<(), Error> {
//!     let signals = Signals::new(&[
//!         signal_hook::SIGHUP,
//!         signal_hook::SIGTERM,
//!         signal_hook::SIGINT,
//!         signal_hook::SIGQUIT,
//! #       signal_hook::SIGUSR1,
//!     ])?;
//! #   // A trick to terminate the example when run as doc-test. Not part of the real code.
//! #   unsafe { libc::kill(libc::getpid(), signal_hook::SIGUSR1) };
//!     'outer: loop {
//!         // Pick up signals that arrived since last time
//!         for signal in signals.pending() {
//!             match signal as libc::c_int {
//!                 signal_hook::SIGHUP => {
//!                     // Reload configuration
//!                     // Reopen the log file
//!                 }
//!                 signal_hook::SIGTERM | signal_hook::SIGINT | signal_hook::SIGQUIT => {
//!                     break 'outer;
//!                 },
//! #               signal_hook::SIGUSR1 => return Ok(()),
//!                 _ => unreachable!(),
//!             }
//!         }
//!         // Do some bit of work ‒ something with upper limit on waiting, so we don't block
//!         // forever with a SIGTERM already waiting.
//!     }
//!     println!("Terminating. Bye bye");
//!     Ok(())
//! }
//! ```

use std::borrow::Borrow;
use std::io::Error;
use std::iter::Enumerate;
use std::os::unix::io::AsRawFd;
#[cfg(not(feature = "mio-support"))]
use std::os::unix::net::UnixStream;
use std::slice::Iter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use libc::{self, c_int};
#[cfg(feature = "mio-support")]
use mio_uds::UnixStream;

use pipe;
use SigId;

/// Maximal signal number we support.
const MAX_SIGNUM: usize = 128;

#[derive(Debug)]
struct Waker {
    pending: Vec<AtomicBool>,
    read: UnixStream,
    write: UnixStream,
}

#[derive(Debug)]
struct RegisteredSignals(Mutex<Vec<Option<SigId>>>);

impl Drop for RegisteredSignals {
    fn drop(&mut self) {
        let lock = self.0.lock().unwrap();
        for id in lock.iter().filter_map(|s| *s) {
            ::unregister(id);
        }
    }
}

/// The main structure of the module, representing interest in some signals.
///
/// Unlike the helpers in other modules, this registers the signals when created and unregisters
/// them on drop. It provides the pending signals during its lifetime, either in batches or as an
/// infinite iterator.
///
/// # Multiple consumers
///
/// You may have noticed this structure can be used simultaneously by multiple threads. If it is
/// done, a signal arrives to one of the threads (on the first come, first serve basis). The signal
/// is *not* broadcasted to all currently active threads.
///
/// A similar thing applies to cloning the structure ‒ at least one of the copies gets the signal,
/// but it is not broadcasted to all of them.
///
/// If you need multiple recipients, you can create multiple independent instances (not by cloning,
/// but by the constructor).
///
/// # Examples
///
/// ```rust
/// # extern crate signal_hook;
/// #
/// # use std::io::Error;
/// # use std::thread;
/// use signal_hook::iterator::Signals;
///
/// #
/// # fn main() -> Result<(), Error> {
/// let signals = Signals::new(&[signal_hook::SIGUSR1, signal_hook::SIGUSR2])?;
/// thread::spawn(move || {
///     for signal in &signals {
///         match signal {
///             signal_hook::SIGUSR1 => {},
///             signal_hook::SIGUSR2 => {},
///             _ => unreachable!(),
///         }
///     }
/// });
/// # Ok(())
/// # }
/// ```
///
/// # `mio` support
///
/// If the crate is compiled with the `mio-support` flag, the `Signals` becomes pluggable into
/// `mio` (it implements the `Evented` trait). If it becomes readable, there may be new signals to
/// pick up. The structure is expected to be registered with level triggered mode.
///
/// # `tokio` support
///
/// If the crate is compiled with the `tokio-support` flag, the [`into_async`](#method.into_async)
/// method becomes available. This method turns the iterator into an asynchronous stream of
/// received signals.
#[derive(Clone, Debug)]
pub struct Signals {
    ids: Arc<RegisteredSignals>,
    waker: Arc<Waker>,
}

impl Signals {
    /// Creates the `Signals` structure.
    ///
    /// This registers all the signals listed. The same restrictions (panics, errors) apply as with
    /// [`register`](../fn.register.html).
    pub fn new<I, S>(signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
    {
        let (read, write) = UnixStream::pair()?;
        let pending = (0..MAX_SIGNUM).map(|_| AtomicBool::new(false)).collect();
        let waker = Arc::new(Waker {
            pending,
            read,
            write,
        });
        let ids = (0..MAX_SIGNUM).map(|_| None).collect();
        let me = Self {
            ids: Arc::new(RegisteredSignals(Mutex::new(ids))),
            waker,
        };
        for sig in signals {
            me.add_signal(*sig.borrow())?;
        }
        Ok(me)
    }

    /// Registers another signal to the set watched by this [`Signals`] instance.
    ///
    /// # Notes
    ///
    /// * This is safe to call concurrently from whatever thread.
    /// * This is *not* safe to call from within a signal handler.
    /// * If the signal number was already registered previously, this is a no-op.
    /// * If this errors, the original set of signals is left intact.
    /// * This actually registers the signal into the whole group of [`Signals`] cloned from each
    ///   other, so any of them might start receiving the signals.
    ///
    /// # Panics
    ///
    /// * If the given signal is [forbidden][::FORBIDDEN].
    /// * If the signal number is negative or larger than internal limit. The limit should be
    ///   larger than any supported signal the OS supports.
    pub fn add_signal(&self, signal: c_int) -> Result<(), Error> {
        assert!(signal >= 0);
        assert!(
            (signal as usize) < MAX_SIGNUM,
            "Signal number {} too large. If your OS really supports such signal, file a bug",
            signal,
        );
        let mut lock = self.ids.0.lock().unwrap();
        // Already registered, ignoring
        if lock[signal as usize].is_some() {
            return Ok(());
        }

        let waker = Arc::clone(&self.waker);
        let action = move || {
            waker.pending[signal as usize].store(true, Ordering::SeqCst);
            pipe::wake(waker.write.as_raw_fd());
        };
        let id = unsafe { ::register(signal, action) }?;
        lock[signal as usize] = Some(id);
        Ok(())
    }

    /// Reads data from the internal self-pipe.
    ///
    /// If `wait` is `true` and there are no data in the self pipe, it blocks until some come.
    ///
    /// Returns weather it successfully read something.
    fn flush(&self, wait: bool) -> bool {
        const SIZE: usize = 1024;
        let mut buff = [0u8; SIZE];
        let res = unsafe {
            // We ignore all errors on purpose. This should not be something like closed file
            // descriptor. It could EAGAIN, but that's OK in case we say MSG_DONTWAIT. If it's
            // EINTR, then it's OK too, it'll only create a spurious wakeup.
            libc::recv(
                self.waker.read.as_raw_fd(),
                buff.as_mut_ptr() as *mut libc::c_void,
                SIZE,
                if wait { 0 } else { libc::MSG_DONTWAIT },
            )
        };
        res > 0
    }

    /// Returns an iterator of already received signals.
    ///
    /// This returns an iterator over all the signal numbers of the signals received since last
    /// time they were read (out of the set registered by this `Signals` instance). Note that they
    /// are returned in arbitrary order and a signal number is returned only once even if it was
    /// received multiple times.
    ///
    /// This method returns immediately (does not block) and may produce an empty iterator if there
    /// are no signals ready.
    pub fn pending(&self) -> Pending {
        self.flush(false);

        Pending(self.waker.pending.iter().enumerate())
    }

    /// Waits for some signals to be available and returns an iterator.
    ///
    /// This is similar to [`pending`](#method.pending). If there are no signals available, it
    /// tries to wait for some to arrive. However, due to implementation details, this still can
    /// produce an empty iterator.
    ///
    /// This can block for arbitrary long time.
    ///
    /// Note that the blocking is done in this method, not in the iterator.
    pub fn wait(&self) -> Pending {
        self.flush(true);

        Pending(self.waker.pending.iter().enumerate())
    }

    /// Returns an infinite iterator over arriving signals.
    ///
    /// The iterator's `next()` blocks as necessary to wait for signals to arrive. This is adequate
    /// if you want to designate a thread solely to handling signals. If multiple signals come at
    /// the same time (between two values produced by the iterator), they will be returned in
    /// arbitrary order. Multiple instances of the same signal may be collated.
    ///
    /// This is also the iterator returned by `IntoIterator` implementation on `&Signals`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate libc;
    /// # extern crate signal_hook;
    /// #
    /// # use std::io::Error;
    /// # use std::thread;
    /// #
    /// use signal_hook::iterator::Signals;
    ///
    /// # fn main() -> Result<(), Error> {
    /// let signals = Signals::new(&[signal_hook::SIGUSR1, signal_hook::SIGUSR2])?;
    /// thread::spawn(move || {
    ///     for signal in signals.forever() {
    ///         match signal {
    ///             signal_hook::SIGUSR1 => {},
    ///             signal_hook::SIGUSR2 => {},
    ///             _ => unreachable!(),
    ///         }
    ///     }
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub fn forever(&self) -> Forever {
        Forever {
            signals: self,
            iter: self.pending(),
        }
    }
}

impl<'a> IntoIterator for &'a Signals {
    type Item = c_int;
    type IntoIter = Forever<'a>;
    fn into_iter(self) -> Forever<'a> {
        self.forever()
    }
}

/// The iterator of one batch of signals.
///
/// This is returned by the [`pending`](struct.Signals.html#method.pending) and
/// [`wait`](struct.Signals.html#method.wait) methods.
pub struct Pending<'a>(Enumerate<Iter<'a, AtomicBool>>);

impl<'a> Iterator for Pending<'a> {
    type Item = c_int;

    fn next(&mut self) -> Option<c_int> {
        while let Some((sig, flag)) = self.0.next() {
            if flag
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return Some(sig as c_int);
            }
        }

        None
    }
}

/// The infinite iterator of signals.
///
/// It is returned by the [`forever`](struct.Signals.html#method.forever) and by the `IntoIterator`
/// implementation of [`&Signals`](struct.Signals.html).
pub struct Forever<'a> {
    signals: &'a Signals,
    iter: Pending<'a>,
}

impl<'a> Iterator for Forever<'a> {
    type Item = c_int;

    fn next(&mut self) -> Option<c_int> {
        loop {
            if let Some(result) = self.iter.next() {
                return Some(result);
            }

            self.iter = self.signals.wait();
        }
    }
}

#[cfg(feature = "mio-support")]
mod mio_support {
    use std::io::Error;

    use mio::event::Evented;
    use mio::{Poll, PollOpt, Ready, Token};

    use super::Signals;

    impl Evented for Signals {
        fn register(
            &self,
            poll: &Poll,
            token: Token,
            interest: Ready,
            opts: PollOpt,
        ) -> Result<(), Error> {
            self.waker.read.register(poll, token, interest, opts)
        }

        fn reregister(
            &self,
            poll: &Poll,
            token: Token,
            interest: Ready,
            opts: PollOpt,
        ) -> Result<(), Error> {
            self.waker.read.reregister(poll, token, interest, opts)
        }

        fn deregister(&self, poll: &Poll) -> Result<(), Error> {
            self.waker.read.deregister(poll)
        }
    }

    #[cfg(test)]
    mod tests {
        use std::time::Duration;

        use libc;
        use mio::Events;

        use super::*;

        #[test]
        fn mio_wakeup() {
            let signals = Signals::new(&[::SIGUSR1]).unwrap();
            let token = Token(0);
            let poll = Poll::new().unwrap();
            poll.register(&signals, token, Ready::readable(), PollOpt::level())
                .unwrap();
            let mut events = Events::with_capacity(10);
            unsafe { libc::kill(libc::getpid(), ::SIGUSR1) };
            poll.poll(&mut events, Some(Duration::from_secs(10)))
                .unwrap();
            let event = events.iter().next().unwrap();
            assert!(event.readiness().is_readable());
            assert_eq!(token, event.token());
            let sig = signals.pending().next().unwrap();
            assert_eq!(::SIGUSR1, sig);
        }
    }
}

#[cfg(feature = "tokio-support")]
mod tokio_support {
    use std::io::Error;
    use std::sync::atomic::Ordering;

    use futures::stream::Stream;
    use futures::{Async as AsyncResult, Poll};
    use libc::{self, c_int};
    use tokio_reactor::{Handle, Registration};

    use super::Signals;

    /// An asynchronous stream of registered signals.
    ///
    /// It is created by converting [`Signals`](struct.Signals.html). See
    /// [`Signals::into_async`](struct.Signals.html#method.into_async).
    ///
    /// # Cloning
    ///
    /// If you register multiple signals, then create multiple `Signals` instances by cloning and
    /// convert them to `Async`, one of them can „steal“ wakeups for several signals at once. This
    /// one will produce the signals while the others will be silent.
    ///
    /// This has an effect if the one consumes them slowly or is dropped after the first one.
    ///
    /// It is recommended not to clone the `Signals` instances and keep just one `Async` stream
    /// around.
    #[derive(Debug)]
    pub struct Async {
        registration: Registration,
        inner: Signals,
        // It seems we can't easily use the iterator into the array here because of lifetimes ‒
        // using non-'static things in around futures is real pain.
        position: usize,
    }

    impl Async {
        /// Creates a new `Async`.
        pub fn new(signals: Signals, handle: &Handle) -> Result<Self, Error> {
            let registration = Registration::new();
            registration.register_with(&signals, handle)?;
            Ok(Async {
                registration,
                inner: signals,
                position: 0,
            })
        }
    }

    impl Stream for Async {
        type Item = libc::c_int;
        type Error = Error;
        fn poll(&mut self) -> Poll<Option<libc::c_int>, Self::Error> {
            loop {
                if self.position >= self.inner.waker.pending.len() {
                    if self.registration.poll_read_ready()?.is_not_ready() {
                        return Ok(AsyncResult::NotReady);
                    }
                    // Non-blocking clean of the pipe
                    while self.inner.flush(false) {}
                    // By now we have an indication there might be some stuff inside the signals,
                    // reset the scanning position
                    self.position = 0;
                }
                assert!(self.position < self.inner.waker.pending.len());
                let sig = &self.inner.waker.pending[self.position];
                let sig_num = self.position;
                self.position += 1;
                if sig
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    // Successfully claimed a signal, return it
                    return Ok(AsyncResult::Ready(Some(sig_num as c_int)));
                }
            }
        }
    }

    impl Signals {
        /// Turns the iterator into an asynchronous stream.
        ///
        /// This allows getting the signals in asynchronous way in a tokio event loop. Available
        /// only if compiled with the `tokio-support` feature enabled.
        ///
        /// # Examples
        ///
        /// ```rust
        /// extern crate libc;
        /// extern crate signal_hook;
        /// extern crate tokio;
        ///
        /// use std::io::Error;
        ///
        /// use signal_hook::iterator::Signals;
        /// use tokio::prelude::*;
        ///
        /// fn main() -> Result<(), Error> {
        ///     let wait_signal = Signals::new(&[signal_hook::SIGUSR1])?
        ///         .into_async()?
        ///         .into_future()
        ///         .map(|sig| assert_eq!(sig.0.unwrap(), signal_hook::SIGUSR1))
        ///         .map_err(|e| panic!("{}", e.0));
        ///     unsafe { libc::kill(libc::getpid(), signal_hook::SIGUSR1) };
        ///     tokio::run(wait_signal);
        ///     Ok(())
        /// }
        /// ```
        pub fn into_async(self) -> Result<Async, Error> {
            Async::new(self, &Handle::current())
        }

        /// Turns the iterator into a stream, tied into a specific tokio reactor.
        pub fn into_async_with_handle(self, handle: &Handle) -> Result<Async, Error> {
            Async::new(self, handle)
        }
    }
}

#[cfg(feature = "tokio-support")]
pub use self::tokio_support::Async;
