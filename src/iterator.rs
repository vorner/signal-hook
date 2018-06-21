//! An iterator over incoming signals.
//!
//! This provides a higher abstraction over the signals, providing a structure
//! ([`Signals`](struct.Signals.html)) able to iterate over the incoming signals.
//!
//! Depending on the features the crate is compiled with, integration with `mio` and `futures` is
//! provided.
//!
//! TODO: The integrations and features, once they exist.
//!
//! TODO: Examples, order, spurious wakeups.

use std::borrow::Borrow;
use std::collections::hash_map::{HashMap, Iter};
use std::io::Error;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use libc::{self, c_int};

use pipe;
use SigId;

#[derive(Debug)]
struct Waker {
    pending: HashMap<c_int, AtomicBool>,
    read: UnixStream,
    write: UnixStream,
}

/// The main structure of the module, representing interest in some signals.
///
/// Unlike the helpers in other modules, this registers the signals when created and unregisters
/// them on drop. It provides the pending signals during its lifetime, either in batches or as an
/// infinite iterator.
///
/// TODO: Code example, note about multiple readers
///
/// # Examples
///
/// ```rust,norun
/// # extern crate libc;
/// # extern crate signal_hook;
/// #
/// # use std::io::Error;
/// # use std::thread;
/// #
/// # fn main() -> Result<(), Error> {
/// let signals = signal_hook::iterator::Signals::new(&[libc::SIGUSR1, libc::SIGUSR2])?;
/// thread::spawn(move || {
///     for signal in &signals {
///         match signal {
///             libc::SIGUSR1 => {},
///             libc::SIGUSR2 => {},
///             _ => unreachable!(),
///         }
///     }
/// });
/// # Ok(())
/// # }
#[derive(Clone, Debug)]
pub struct Signals {
    ids: Vec<SigId>,
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
        let pending = signals
            .into_iter()
            .map(|sig| (*sig.borrow(), AtomicBool::new(false)))
            .collect();
        let waker = Arc::new(Waker {
            pending,
            read,
            write,
        });
        let ids = waker
            .pending
            .keys()
            .map(|sig| {
                let sig = *sig;
                let waker = Arc::clone(&waker);
                let action = move || {
                    waker.pending[&sig].store(true, Ordering::SeqCst);
                    pipe::wake(waker.write.as_raw_fd());
                };
                unsafe { ::register(sig, action) }
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { ids, waker })
    }

    /// Reads data from the internal self-pipe.
    ///
    /// If `wait` is `true` and there are no data in the self pipe, it blocks until some come.
    fn flush(&self, wait: bool) {
        const SIZE: usize = 1024;
        let mut buff = [0u8; SIZE];
        unsafe {
            // We ignore all errors on purpose. This should not be something like closed file
            // descriptor. It could EAGAIN, but that's OK in case we say MSG_DONTWAIT. If it's
            // EINTR, then it's OK too, it'll only create a spurious wakeup.
            libc::recv(
                self.waker.read.as_raw_fd(),
                buff.as_mut_ptr() as *mut libc::c_void,
                SIZE,
                if wait { 0 } else { libc::MSG_DONTWAIT },
            );
        }
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

        Pending(self.waker.pending.iter())
    }

    /// Waits for some signals to be available and returns an iterator.
    ///
    /// This is similar to [`pending`](#method.pending). If there are no signals available, it
    /// tries to wait for some to arrive. However, due to implementation details, this still can
    /// produce an empty iterator.
    ///
    /// This can block for arbitrary length.
    ///
    /// Note that the blocking is done in this method, not in the iterator.
    pub fn wait(&self) -> Pending {
        self.flush(true);

        Pending(self.waker.pending.iter())
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
    /// ```rust,norun
    /// # extern crate libc;
    /// # extern crate signal_hook;
    /// #
    /// # use std::io::Error;
    /// # use std::thread;
    /// #
    /// # fn main() -> Result<(), Error> {
    /// let signals = signal_hook::iterator::Signals::new(&[libc::SIGUSR1, libc::SIGUSR2])?;
    /// thread::spawn(move || {
    ///     for signal in signals.forever() {
    ///         match signal {
    ///             libc::SIGUSR1 => {},
    ///             libc::SIGUSR2 => {},
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

impl Drop for Signals {
    fn drop(&mut self) {
        for id in &self.ids {
            ::unregister(*id);
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
pub struct Pending<'a>(Iter<'a, c_int, AtomicBool>);

impl<'a> Iterator for Pending<'a> {
    type Item = c_int;

    fn next(&mut self) -> Option<c_int> {
        while let Some((sig, flag)) = self.0.next() {
            if flag.swap(false, Ordering::SeqCst) {
                return Some(*sig);
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
