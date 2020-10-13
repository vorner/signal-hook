//! An iterator over incoming signals.
//!
//! This provides a higher abstraction over the signals, providing a structure
//! ([`Signals`](struct.Signals.html)) able to iterate over the incoming signals.
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
//! #   unsafe { libc::raise(signal_hook::SIGUSR1) };
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

pub mod backend;

use std::borrow::Borrow;
use std::io::{Error, ErrorKind, Read};
use std::os::unix::net::UnixStream;

use libc::{self, c_int};

use self::backend::PollResult;
pub use self::backend::{Pending, SignalDelivery, SignalIterator};

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
#[derive(Debug, Clone)]
pub struct Signals(SignalDelivery<UnixStream, UnixStream>);

type Forever = SignalIterator<UnixStream, UnixStream>;

impl Signals {
    /// Creates the `Signals` structure.
    ///
    /// This registers all the signals listed. The same restrictions (panics, errors) apply as
    /// with [`add_signal`](#method.add_signal).
    pub fn new<I, S>(signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
    {
        let (read, write) = UnixStream::pair()?;
        Ok(Signals(backend::SignalDelivery::with_pipe(
            read, write, signals,
        )?))
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
    /// * If the given signal is [forbidden][crate::FORBIDDEN].
    /// * If the signal number is negative or larger than internal limit. The limit should be
    ///   larger than any supported signal the OS supports.
    pub fn add_signal(&self, signal: c_int) -> Result<(), Error> {
        self.0.add_signal(signal)
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
        self.0.pending()
    }

    /// Block until the stream contains some bytes.
    ///
    /// Returns true if it was possible to read a byte and false otherwise.
    fn has_signals(mut read: &UnixStream) -> Result<bool, Error> {
        loop {
            match read.read(&mut [0u8]) {
                Ok(num_read) => break Ok(num_read > 0),
                // If we get an EINTR error it is fine to retry reading from the stream.
                // Otherwise we should pass on the error to the caller.
                Err(error) => {
                    if error.kind() != ErrorKind::Interrupted {
                        break Err(error);
                    }
                }
            }
        }
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
        match self.0.poll_pending(&mut Self::has_signals) {
            Ok(Some(pending)) => pending,
            // Because of the blocking has_signals method the poll_pending method
            // only returns None if the instance is closed. But we want to return
            // a possibly empty pending object anyway.
            Ok(None) => self.pending(),
            // Users can't manipulate the internal file descriptors and the way we use them
            // shouldn't produce any errors. So it is OK to panic.
            Err(error) => panic!("Unexpected error: {}", error),
        }
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
    /// This iterator terminates only if the [`Signals`] is explicitly [closed][Signals::close].
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
        Forever::new(self.0.clone())
    }

    /// Is it closed?
    ///
    /// See [`close`][Signals::close].
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Closes the instance.
    ///
    /// This is meant to signalize termination through all the interrelated instances
    /// (e. g. cloning a [`Signals`] instance). After calling close:
    ///
    /// * [`is_closed`][Signals::is_closed] will return true.
    /// * All currently blocking operations on all threads and all the instances are interrupted
    ///   and terminate.
    /// * Any further operations will never block.
    /// * Further signals may or may not be returned from the iterators. However, if any are
    ///   returned, these are real signals that happened.
    /// * The [`forever`][Signals::forever] terminates (follows from the above).
    ///
    /// The goal is to be able to shut down any background thread that handles only the signals.
    ///
    /// ```rust
    /// # use signal_hook::iterator::Signals;
    /// # use signal_hook::SIGUSR1;
    /// # fn main() -> Result<(), std::io::Error> {
    /// let signals = Signals::new(&[SIGUSR1])?;
    /// let signals_bg = signals.clone();
    /// let thread = std::thread::spawn(move || {
    ///     for signal in &signals_bg {
    ///         // Whatever with the signal
    /// #       let _ = signal;
    ///     }
    /// });
    ///
    /// signals.close();
    ///
    /// // The thread will terminate on its own now (the for cycle runs out of signals).
    /// thread.join().expect("background thread panicked");
    /// # Ok(()) }
    /// ```
    pub fn close(&self) {
        self.0.close()
    }
}

impl<'a> IntoIterator for &'a Signals {
    type Item = c_int;
    type IntoIter = Forever;
    fn into_iter(self) -> Self::IntoIter {
        self.forever()
    }
}

impl Iterator for Forever {
    type Item = c_int;

    fn next(&mut self) -> Option<c_int> {
        match self.poll_signal(&mut Signals::has_signals) {
            PollResult::Signal(result) => Some(result),
            PollResult::Closed => None,
            PollResult::Pending => unreachable!(
                "Because of the blocking has_signals method the \
                poll_signal method never returns Poll::Pending but blocks until a signal arrived"
            ),
            // Users can't manipulate the internal file descriptors and the way we use them
            // shouldn't produce any errors. So it is OK to panic.
            PollResult::Err(error) => panic!("Unexpected error: {}", error),
        }
    }
}
