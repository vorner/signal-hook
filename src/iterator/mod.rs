//! An iterator over incoming signals.
//!
//! This provides a higher abstraction over the signals, providing
//! the [`Signals`] structure which is able to iterate over the
//! incoming signals.
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
//!     let mut signals = Signals::new(&[
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
pub mod exfiltrator;

use std::borrow::Borrow;
use std::io::{Error, ErrorKind, Read};
use std::os::unix::net::UnixStream;

use libc::{self, c_int};

pub use self::backend::{Handle, Pending};
use self::backend::{PollResult, SignalDelivery, SignalIterator};

/// The main structure of the module, representing interest in some signals.
///
/// Unlike the helpers in other modules, this registers the signals when created and unregisters
/// them on drop. It provides the pending signals during its lifetime, either in batches or as an
/// infinite iterator.
///
/// # Multiple threads
///
/// Instances of this struct can be [sent][std::marker::Send] to other threads. In a multithreaded
/// application this can be used to dedicate a separate thread for signal handling. In this case
/// you should get a [`Handle`] using the [`handle`][Signals::handle] method before sending the
/// `Signals` instance to a background thread. With the handle you will be able to shut down the
/// background thread later.
///
/// The controller handle can be shared between as many threads as you like using its
/// [`clone`][Handle::clone] method.
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
/// let handle = signals.handle();
/// thread::spawn(move || {
///     for signal in signals {
///         match signal {
///             signal_hook::SIGUSR1 => {},
///             signal_hook::SIGUSR2 => {},
///             _ => unreachable!(),
///         }
///     }
/// });
/// handle.close();
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Signals(SignalDelivery<UnixStream>);

impl Signals {
    /// Creates the `Signals` structure.
    ///
    /// This registers all the signals listed. The same restrictions (panics, errors) apply as
    /// for the [`Handle::add_signal`] method.
    pub fn new<I, S>(signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
    {
        let (read, write) = UnixStream::pair()?;
        Ok(Signals(SignalDelivery::with_pipe(read, write, signals)?))
    }

    /// Registers another signal to the set watched by this [`Signals`] instance.
    ///
    /// The same restrictions (panics, errors) apply as for the [`Handle::add_signal`]
    /// method.
    pub fn add_signal(&self, signal: c_int) -> Result<(), Error> {
        self.handle().add_signal(signal)
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
    pub fn pending(&mut self) -> Pending {
        self.0.pending()
    }

    /// Block until the stream contains some bytes.
    ///
    /// Returns true if it was possible to read a byte and false otherwise.
    fn has_signals(read: &mut UnixStream) -> Result<bool, Error> {
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
    /// This is similar to [`pending`][Signals::pending]. If there are no signals available, it
    /// tries to wait for some to arrive. However, due to implementation details, this still can
    /// produce an empty iterator.
    ///
    /// This can block for arbitrary long time. If the [`Handle::close`] method is used in
    /// another thread this method will return immediately.
    ///
    /// Note that the blocking is done in this method, not in the iterator.
    pub fn wait(&mut self) -> Pending {
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

    /// Is it closed?
    ///
    /// See [`close`][Handle::close].
    pub fn is_closed(&self) -> bool {
        self.handle().is_closed()
    }

    /// Consume this instance and return an infinite iterator over arriving signals.
    ///
    /// The iterator's `next()` blocks as necessary to wait for signals to arrive. This is adequate
    /// if you want to designate a thread solely to handling signals. If multiple signals come at
    /// the same time (between two values produced by the iterator), they will be returned in
    /// arbitrary order. Multiple instances of the same signal may be collated.
    ///
    /// This is also the iterator returned by `IntoIterator` implementation on `Signals`.
    ///
    /// This iterator terminates only if explicitly [closed][Handle::close].
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
    /// let handle = signals.handle();
    /// thread::spawn(move || {
    ///     for signal in signals.forever() {
    ///         match signal {
    ///             signal_hook::SIGUSR1 => {},
    ///             signal_hook::SIGUSR2 => {},
    ///             _ => unreachable!(),
    ///         }
    ///     }
    /// });
    /// handle.close();
    /// # Ok(())
    /// # }
    /// ```
    pub fn forever(self) -> Forever {
        Forever(SignalIterator::new(self.0))
    }

    /// Get a shareable handle to a [`Handle`] for this instance.
    ///
    /// This can be used to add further signals or close the [`Signals`] instance.
    pub fn handle(&self) -> Handle {
        self.0.handle()
    }
}

impl IntoIterator for Signals {
    type Item = c_int;
    type IntoIter = Forever;
    fn into_iter(self) -> Self::IntoIter {
        self.forever()
    }
}

/// An infinit iterator of arriving signals.
pub struct Forever(SignalIterator<UnixStream>);

impl Forever {
    /// Consume this iterator and return the inner
    /// [`Signals`] instance.
    pub fn into_inner(self) -> Signals {
        Signals(self.0.into_inner())
    }
}

impl Iterator for Forever {
    type Item = c_int;

    fn next(&mut self) -> Option<c_int> {
        match self.0.poll_signal(&mut Signals::has_signals) {
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
