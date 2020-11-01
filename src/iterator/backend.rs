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
use std::fmt::Debug;
use std::io::Error;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use libc::{self, c_int};

use crate::pipe::{self, WakeMethod};
use crate::SigId;

/// Maximal signal number we support.
const MAX_SIGNUM: usize = 128;

trait SelfPipeWrite: Debug + Send + Sync {
    fn wake_readers(&self);
}

impl<W: AsRawFd + Debug + Send + Sync> SelfPipeWrite for W {
    fn wake_readers(&self) {
        pipe::wake(self.as_raw_fd(), WakeMethod::Send);
    }
}

#[derive(Debug)]
struct DeliveryState {
    closed: AtomicBool,
    registered_signal_ids: Mutex<Vec<Option<SigId>>>,
}

impl DeliveryState {
    fn new() -> Self {
        let ids = (0..MAX_SIGNUM).map(|_| None).collect();
        Self {
            closed: AtomicBool::new(false),
            registered_signal_ids: Mutex::new(ids),
        }
    }
}

impl Drop for DeliveryState {
    fn drop(&mut self) {
        let lock = self.registered_signal_ids.lock().unwrap();
        for id in lock.iter().filter_map(|s| *s) {
            crate::unregister(id);
        }
    }
}

/// A struct to control an instance of an associated type
/// (like for example [`Signals`][super::Signals]).
///
/// It allows to register more signal handlers and to shutdown the signal
/// delivery. You can [`clone`][Handle::clone] this type which isn't a
/// very expensive operation. The cloned instances can be shared between
/// multiple threads.
#[derive(Debug, Clone)]
pub struct Handle {
    pending: Arc<[AtomicBool]>,
    write: Arc<dyn SelfPipeWrite>,
    delivery_state: Arc<DeliveryState>,
}

impl Handle {
    fn new<W>(write: W) -> Self
    where
        W: 'static + SelfPipeWrite,
    {
        let pending: Arc<[AtomicBool]> = (0..MAX_SIGNUM)
            .map(|_| AtomicBool::new(false))
            .collect::<Vec<AtomicBool>>()
            .into();

        Self {
            pending,
            write: Arc::new(write),
            delivery_state: Arc::new(DeliveryState::new()),
        }
    }

    /// Registers another signal to the set watched by the associated instance.
    ///
    /// # Notes
    ///
    /// * This is safe to call concurrently from whatever thread.
    /// * This is *not* safe to call from within a signal handler.
    /// * If the signal number was already registered previously, this is a no-op.
    /// * If this errors, the original set of signals is left intact.
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
        let mut lock = self.delivery_state.registered_signal_ids.lock().unwrap();
        // Already registered, ignoring
        if lock[signal as usize].is_some() {
            return Ok(());
        }

        let pending = Arc::clone(&self.pending);
        let write = Arc::clone(&self.write);
        let action = move || {
            pending[signal as usize].store(true, Ordering::SeqCst);
            write.wake_readers();
        };
        let id = unsafe { crate::register(signal, action) }?;
        lock[signal as usize] = Some(id);
        Ok(())
    }

    /// Closes the associated instance.
    ///
    /// This is meant to signalize termination of the signal delivery process.
    /// After calling close:
    ///
    /// * [`is_closed`][Handle::is_closed] will return true.
    /// * All currently blocking operations of associated instances
    ///   are interrupted and terminate.
    /// * Any further operations will not block.
    /// * Further signals may or may not be returned from the iterators. However, if any are
    ///   returned, these are real signals that happened.
    ///
    /// The goal is to be able to shut down any background thread that handles only the signals.
    pub fn close(&self) {
        self.delivery_state.closed.store(true, Ordering::SeqCst);
        self.write.wake_readers();
    }

    /// Is it closed?
    ///
    /// See [`close`][Handle::close].
    pub fn is_closed(&self) -> bool {
        self.delivery_state.closed.load(Ordering::SeqCst)
    }
}

/// A struct for delivering received signals to the main program flow.
/// The self-pipe IO type is generic. See the
/// [`with_pipe`][SignalDelivery::with_pipe] method for requirements
/// for the IO type.
#[derive(Debug)]
pub struct SignalDelivery<R> {
    read: R,
    handle: Handle,
}

impl<R> SignalDelivery<R>
where
    R: 'static + AsRawFd + Send + Sync,
{
    /// Creates the `SignalDelivery` structure.
    ///
    /// The read and write arguments must be the ends of a suitable pipe type. These are used
    /// for communication between the signal handler and main program flow.
    ///
    /// Registers all the signals listed. The same restrictions (panics, errors) apply as with
    /// [`add_signal`][Handle::add_signal].
    ///
    /// # Requirements for the pipe type
    ///
    /// * Must support [`send`](https://man7.org/linux/man-pages/man2/send.2.html) for
    ///   asynchronously writing bytes to the write end
    /// * Must support [`recv`](https://man7.org/linux/man-pages/man2/recv.2.html) for
    ///   reading bytes from the read end
    ///
    /// So UnixStream is a good choice for this.
    pub fn with_pipe<I, S, W>(read: R, write: W, signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
        W: 'static + AsRawFd + Debug + Send + Sync,
    {
        let handle = Handle::new(write);
        let me = Self { read, handle };
        for sig in signals {
            me.handle.add_signal(*sig.borrow())?;
        }
        Ok(me)
    }

    /// Get a reference to the read end of the self pipe
    ///
    /// You may use this method to register the underlying file descriptor
    /// with an eventing system (e. g. epoll) to get notified if there are
    /// bytes in the pipe. If the event system reports the file descriptor
    /// ready for reading you can then call [`pending`][SignalDelivery::pending]
    /// to get the arrived signals.
    pub fn get_read(&self) -> &R {
        &self.read
    }

    /// Get a mutable reference to the read end of the self pipe
    ///
    /// See the [`get_read`][SignalDelivery::get_read] method for some additional
    /// information.
    pub fn get_read_mut(&mut self) -> &mut R {
        &mut self.read
    }

    /// Drains all data from the internal self-pipe. This method will never block.
    fn flush(&mut self) {
        const SIZE: usize = 1024;
        let mut buff = [0u8; SIZE];

        unsafe {
            // Draining the data in the self pipe. We ignore all errors on purpose. This
            // should not be something like closed file descriptor. It could EAGAIN, but
            // that's OK in case we say MSG_DONTWAIT. If it's EINTR, then it's OK too,
            // it'll only create a spurious wakeup.
            while libc::recv(
                self.read.as_raw_fd(),
                buff.as_mut_ptr() as *mut libc::c_void,
                SIZE,
                libc::MSG_DONTWAIT,
            ) > 0
            {}
        }
    }

    /// Returns an iterator of already received signals.
    ///
    /// This returns an iterator over all the signal numbers of the signals received since last
    /// time they were read (out of the set registered by this `SignalDelivery` instance). Note
    /// that they are returned in arbitrary order and a signal number is returned only once even
    /// if it was received multiple times.
    ///
    /// This method returns immediately (does not block) and may produce an empty iterator if
    /// there are no signals ready.
    pub fn pending(&mut self) -> Pending {
        self.flush();
        Pending::new(Arc::clone(&self.handle.pending))
    }

    /// Checks the reading end of the self pipe for available signals.
    ///
    /// If there are no signals available or this instance was already closed it returns
    /// [`Option::None`]. If there are some signals it returns a [`Pending`](struct.Pending)
    /// instance wrapped inside a [`Option::Some`]. However, due to implementation details,
    /// this still can produce an empty iterator.
    ///
    /// This method doesn't check the reading end by itself but uses the passed in callback.
    /// This method blocks if and only if the callback blocks trying to read some bytes.
    pub fn poll_pending<F>(&mut self, has_signals: &mut F) -> Result<Option<Pending>, Error>
    where
        F: FnMut(&mut R) -> Result<bool, Error>,
    {
        if self.handle.is_closed() {
            return Ok(None);
        }

        match has_signals(self.get_read_mut()) {
            Ok(false) => Ok(None),
            Ok(true) => Ok(Some(self.pending())),
            Err(err) => Err(err),
        }
    }

    /// Get a [`Handle`] for this `SignalDelivery` instance.
    ///
    /// This can be used to add further signals or close the whole
    /// signal delivery mechanism.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

/// The iterator of one batch of signals.
///
/// This is returned by the [`pending`][SignalDelivery::pending] method.
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
pub struct SignalIterator<R> {
    signals: SignalDelivery<R>,
    iter: Pending,
}

impl<R> SignalIterator<R>
where
    R: 'static + AsRawFd + Send + Sync,
{
    /// Create a new infinite iterator for signals registered with the passed
    /// in [`SignalDelivery`] instance.
    pub fn new(mut signals: SignalDelivery<R>) -> Self {
        let iter = signals.pending();
        Self { signals, iter }
    }

    /// Return a signal if there is one or tell the caller that there is none at the moment.
    ///
    /// You have to pass in a callback which checks the underlying reading end of the pipe if
    /// there may be any pending signals. This callback may or may not block. If the callback
    /// returns [`true`] this method will try to fetch the next signal and return it as a
    /// [`PollResult::Signal`]. If the callback returns [`false`] the method will return
    /// [`PollResult::Pending`] and assume it will be called again at a later point in time.
    /// The callback may be called any number of times by this function.
    ///
    /// If the iterator was closed by the [`close`][Handle::close] method of the associtated
    /// [`Handle`] this method will return [`PollResult::Closed`].
    pub fn poll_signal<F>(&mut self, has_signals: &mut F) -> PollResult
    where
        F: FnMut(&mut R) -> Result<bool, Error>,
    {
        // The loop is necessary because it is possible that a signal was already consumed
        // by a previous pending iterator due to the asynchronous nature of signals and
        // always moving to the end of the iterator before calling has_more.
        while !self.signals.handle.is_closed() {
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

    /// Get a shareable [`Handle`] for this instance.
    ///
    /// This can be used to add further signals or terminate the whole
    /// signal iteration using the [`close`][Handle::close] method.
    pub fn handle(&self) -> Handle {
        self.signals.handle()
    }

    /// Consume this iterator and return the inner
    /// [`SignalDelivery`] instance.
    pub fn into_inner(self) -> SignalDelivery<R> {
        self.signals
    }
}
