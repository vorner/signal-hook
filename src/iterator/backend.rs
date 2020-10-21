//! A backend module for implementing the iterator like
//! [`iterator`][crate::iterator] module and the asynchronous
//! adapter crates.
//!
//! This module contains generic types which abstract over the concrete
//! IO type for the self-pipe. The motivation for having this abstraction
//! are the adapter crates for different asynchronous runtimes. The runtimes
//! provide their own wrappers for [`std::os::unix::net::UnixStream`]
//! which should be used as the internal self pipe. But large parts of the
//! remaining functionality doesn't depend directly onto the IO type and can
//! be reused.
//!
//! See also the [`SignalDelivery::with_pipe`] method for more information
//! about requirements the IO types have to fulfill.
//!
//! As a regular user you shouldn't need to use the types in this module.
//! Use the [`Signals`][crate::iterator::Signals] struct or one of the types
//! contained in the adapter libraries instead.

use std::borrow::Borrow;
use std::io::Error;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use libc::{self, c_int};

use crate::pipe::{self, WakeMethod};
use crate::SigId;

/// Maximal signal number we support.
const MAX_SIGNUM: usize = 128;

#[derive(Debug)]
struct Waker<R, W> {
    pending: Arc<[AtomicBool]>,
    closed: AtomicBool,
    read: R,
    write: W,
}

impl<R, W: AsRawFd> Waker<R, W> {
    /// Create a new instance for the given read and write pipe ends.
    fn new(read: R, write: W) -> Self {
        let pending: Arc<[AtomicBool]> = (0..MAX_SIGNUM)
            .map(|_| AtomicBool::new(false))
            .collect::<Vec<AtomicBool>>()
            .into();

        Self {
            pending,
            closed: AtomicBool::new(false),
            read,
            write,
        }
    }

    /// Get a [`Pending`](struct.Pending) instance for iterating through recieved signals.
    fn pending(&self) -> Pending {
        Pending::new(Arc::clone(&self.pending))
    }

    /// Sends a wakeup signal to the internal wakeup pipe.
    fn wake(&self) {
        pipe::wake(self.write.as_raw_fd(), WakeMethod::Send, None);
    }
}

#[derive(Debug)]
struct RegisteredSignals(Mutex<Vec<Option<SigId>>>);

impl Drop for RegisteredSignals {
    fn drop(&mut self) {
        let lock = self.0.lock().unwrap();
        for id in lock.iter().filter_map(|s| *s) {
            crate::unregister(id);
        }
    }
}

/// A struct for delivering received signals to the main program flow.
/// The self-pipe IO type is generic. See the [with_pipe](#method.with_pipe)
/// method for requirements for the IO type.
#[derive(Debug)]
pub struct SignalDelivery<R, W> {
    ids: Arc<RegisteredSignals>,
    waker: Arc<Waker<R, W>>,
}

// We have to implement Clone by hand due to a known issue of
// derive(Clone) for generic structs.
impl<R, W> Clone for SignalDelivery<R, W> {
    fn clone(&self) -> Self {
        Self {
            ids: Arc::clone(&self.ids),
            waker: Arc::clone(&self.waker),
        }
    }
}

impl<R, W> SignalDelivery<R, W>
where
    R: 'static + AsRawFd + Send + Sync,
    W: 'static + AsRawFd + Send + Sync,
{
    /// Creates the `SignalDelivery` structure.
    ///
    /// The read and write arguments must be the ends of a suitable pipe type. These are used
    /// for communication between the signal handler and main program flow.
    ///
    /// Registers all the signals listed. The same restrictions (panics, errors) apply as with
    /// [`add_signal`](#method.add_signal).
    ///
    /// # Requirements for the pipe type
    ///
    /// * Must support [`send`](https://man7.org/linux/man-pages/man2/send.2.html) for
    ///   asynchronously writing bytes to the write end
    /// * Must support [`recv`](https://man7.org/linux/man-pages/man2/recv.2.html) for
    ///   reading bytes from the read end
    ///
    /// So UnixStream is a good choice for this.
    pub fn with_pipe<I, S>(read: R, write: W, signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
    {
        let waker = Arc::new(Waker::new(read, write));
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
    /// * This actually registers the signal into the whole group of [`SignalDelivery`]
    ///   cloned from each other, so any of them might start receiving the signals.
    ///
    /// # Panics
    ///
    /// * If the given signal is [forbidden][crate::FORBIDDEN].
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
            waker.wake();
        };
        let id = unsafe { crate::register(signal, action) }?;
        lock[signal as usize] = Some(id);
        Ok(())
    }

    /// Get a reference to the read end of the self pipe
    ///
    /// You may use this method to register the underlying file descriptor
    /// with an eventing system (e. g. epoll) to get notified if there are
    /// bytes in the pipe. If the event system reports the file descriptor
    /// ready for reading you can then call [`pending`][SignalDelivery::pending]
    /// to get the arrived signals.
    ///
    /// # Warning
    ///
    /// Do not use the reference to read from it directly. Reading from the
    /// pipe should always be done through the [`pending`][SignalDelivery::pending]
    /// or [`poll_pending`][SignalDelivery::poll_pending] methods. Those methods
    /// contain a special procedure to wake other cloned instances which are currently
    /// reading from the pipe. Reading from the pipe directly will most likely break
    /// this shutdown protocol.
    pub fn get_read(&self) -> &R {
        &self.waker.read
    }

    /// Drains all data from the internal self-pipe. This method will never block.
    fn flush(&self) {
        const SIZE: usize = 1024;
        let mut buff = [0u8; SIZE];

        unsafe {
            // Draining the data in the self pipe. We ignore all errors on purpose. This
            // should not be something like closed file descriptor. It could EAGAIN, but
            // that's OK in case we say MSG_DONTWAIT. If it's EINTR, then it's OK too,
            // it'll only create a spurious wakeup.
            while libc::recv(
                self.waker.read.as_raw_fd(),
                buff.as_mut_ptr() as *mut libc::c_void,
                SIZE,
                libc::MSG_DONTWAIT,
            ) > 0
            {}
        }

        if self.is_closed() {
            // Wake any other sleeping ends
            // (if none wait, it'll only leave garbage inside the pipe, but we'll close it soon
            // anyway).
            self.waker.wake();
        }
    }

    /// Returns an iterator of already received signals.
    ///
    /// This returns an iterator over all the signal numbers of the signals received since last
    /// time they were read (out of the set registered by this `Signals` instance). Note that
    /// they are returned in arbitrary order and a signal number is returned only once even if
    /// it was received multiple times.
    ///
    /// This method returns immediately (does not block) and may produce an empty iterator if
    /// there are no signals ready.
    pub fn pending(&self) -> Pending {
        if !self.is_closed() {
            self.flush();
        }
        self.waker.pending()
    }

    /// Checks the reading end of the self pipe for available signals.
    ///
    /// If there are no signals available or this instance was already closed it returns
    /// [`Option::None`]. If there are some signals it returns a [`Pending`](struct.Pending)
    /// instance wrapped inside a [`Option::Some`]. However, due to implementation details,
    /// this still can produce an empty iterator.
    ///
    /// This method doesn't check the reading end itself but uses the passed in callback. This
    /// method blocks if and only if the callback blocks trying to read some bytes.
    pub fn poll_pending<F>(&self, has_signals: &mut F) -> Result<Option<Pending>, Error>
    where
        F: FnMut(&R) -> Result<bool, Error>,
    {
        if self.is_closed() {
            return Ok(None);
        }

        match has_signals(self.get_read()) {
            Ok(false) => Ok(None),
            Ok(true) => {
                self.flush();
                Ok(Some(self.waker.pending()))
            }
            Err(err) => Err(err),
        }
    }

    /// Is it closed?
    ///
    /// See [`close`][#method.close].
    pub fn is_closed(&self) -> bool {
        self.waker.closed.load(Ordering::SeqCst)
    }

    /// Closes the instance.
    ///
    /// This is meant to signalize termination through all the interrelated instances ‒ the
    /// ones created by cloning the same original [`SignalDelivery`] instance. After calling
    /// close:
    ///
    /// * [`is_closed`][#method.is_closed] will return true.
    /// * All currently blocking operations on all threads and all the instances are
    ///   interrupted and terminate.
    /// * Any further operations will not block.
    /// * Further signals may or may not be returned from the iterators. However, if any are
    ///   returned, these are real signals that happened.
    ///
    /// The goal is to be able to shut down any background thread that handles only the signals.
    pub fn close(&self) {
        self.waker.closed.store(true, Ordering::SeqCst);
        self.waker.wake();
    }
}

/// The iterator of one batch of signals.
///
/// This is returned by the [`pending`](struct.SignalDelivery.html#method.pending) method.
pub struct Pending {
    pending: Arc<[AtomicBool]>,
    position: usize,
}

impl Pending {
    fn new(pending: Arc<[AtomicBool]>) -> Self {
        Self {
            pending,
            position: 0,
        }
    }
}

impl Iterator for Pending {
    type Item = c_int;

    fn next(&mut self) -> Option<c_int> {
        while self.position < self.pending.len() {
            let sig = self.position;
            let flag = &self.pending[sig];
            self.position += 1;
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

/// Possible results of the [`poll_signal`][SignalIterator::poll_signal] function.
pub enum PollResult {
    /// A signal arrived
    Signal(c_int),
    /// There are no signals yet but there may arrive some in the future
    Pending,
    /// The iterator was closed. There won't be any signals reported from now on.
    Closed,
    /// An error happened during polling for arrived signals.
    Err(Error),
}

/// An infinite iterator of received signals.
pub struct SignalIterator<R, W> {
    signals: SignalDelivery<R, W>,
    iter: Pending,
}

impl<R, W> SignalIterator<R, W>
where
    R: 'static + AsRawFd + Send + Sync,
    W: 'static + AsRawFd + Send + Sync,
{
    /// Create a new infinite iterator for signals registered with the passed
    /// in [`SignalDelivery`](struct.SignalDelivery) instance.
    pub fn new(signals: SignalDelivery<R, W>) -> Self {
        let iter = signals.pending();
        Self { signals, iter }
    }

    /// Return a signal if there is one or tell the caller that there is none at the moment.
    ///
    /// You have to pass in a callback which checks the underlying reading end of the pipe if
    /// there may be any pending signals. This callback may or may not block. If there are
    /// more signals the callback should return a [`Pending`](struct.Pending) instance wrapped
    /// inside a [`Option::Some`]. If the callback returns [`Option::Some`] this method will
    /// try to fetch the next signal from the [`Pending`] iterator and return it as
    /// [`Poll::Ready`]. If the callback returns [`Option::None`] the method will return
    /// [`Poll::Pending`] and assume it will be called again at a later point in time. The
    /// callback may be called any number of times by this function.
    pub fn poll_signal<F>(&mut self, has_signals: &mut F) -> PollResult
    where
        F: FnMut(&R) -> Result<bool, Error>,
    {
        // The loop is necessary because there could be multiple Signal instances querying
        // and resetting the same set of signal flags. So even if the has_more callback
        // returned true it could be that the pending iterator doesn't return anything
        // because an instance in another thread already consumed the signals in the
        // meantime. It is also possible that the signal was already consumed by the
        // previous pending iterator due to the asynchronous nature of signals and always
        // moving to the end of the iterator before calling has_more.
        while !self.signals.is_closed() {
            if let Some(result) = self.iter.next() {
                return PollResult::Signal(result);
            }

            match self.signals.poll_pending(has_signals) {
                Ok(Some(pending)) => self.iter = pending,
                Ok(None) => return PollResult::Pending,
                Err(err) => return PollResult::Err(err),
            }
        }

        PollResult::Closed
    }
}
