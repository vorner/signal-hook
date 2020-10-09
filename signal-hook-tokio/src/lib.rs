#![doc(
    html_root_url = "https://docs.rs/signal-hook-tokio/0.1.0/",
    test(attr(deny(warnings)))
)]
#![warn(missing_docs)]
//! A crate for integrating signal handling with the Tokio runtime.
//!
//! There are different sub modules for supporting different Tokio
//! versions. The support for a version must be activated by a
//! feature flag:
//!
//! * `support-v0_1` for sub module [`v0_1`]
//!
//! See the specific sub modules for usage examples.

/// A module for integrating signal handling with the Tokio 0.1 runtime.
///
/// This provides the [`Signals`][v0_1::Signals] struct which acts as a
/// [`Stream`][`tokio_0_1::prelude::Stream`] of signals.
///
/// # Example
///
/// ```rust
/// # use tokio_0_1 as tokio;
/// use std::io::Error;
///
/// use signal_hook_tokio::v0_1::Signals;
/// use tokio::prelude::*;
///
/// enum SignalResult {
///     Err(Error),
///     Shutdown,
/// }
///
/// fn main() -> Result<(), Error> {
///     let signals = Signals::new(&[
///             signal_hook::SIGTERM,
/// #           signal_hook::SIGUSR1,
///         ])?
///         .map_err(|error| {
///             SignalResult::Err(error)
///         })
///         .for_each(|sig| {
///             // Return an error to stop the for_each iteration.
///             match sig {
///                 signal_hook::SIGTERM => return future::err(SignalResult::Shutdown),
/// #               signal_hook::SIGUSR1 => return future::err(SignalResult::Shutdown),
///                 _ => unreachable!(),
///             }
///         })
///         .map_err(|reason| {
///             if let SignalResult::Err(error) = reason {
///                 eprintln!("Internal error: {}", error);
///             }
///         });
///
/// #   unsafe { libc::raise(signal_hook::SIGUSR1) };
///     tokio::run(signals);
///     Ok(())
/// }
/// ```
#[cfg(feature = "support-v0_1")]
pub mod v0_1 {

    use std::borrow::Borrow;
    use std::io::Error;
    use std::sync::Arc;

    use signal_hook::iterator::backend::{Controller, PollResult, SignalDelivery, SignalIterator};

    use futures_0_1::stream::Stream;
    use futures_0_1::{Async, Poll};

    use libc::{self, c_int};

    use tokio_0_1::io::AsyncRead;
    use tokio_0_1::net::unix::UnixStream;

    /// A shareable handle to control an associated [`Signals`] instance.
    pub type ControllerHandle = Arc<Controller<UnixStream>>;

    /// An asynchronous stream of arriving signals using the tokio runtime.
    ///
    /// The stream doesn't return the signals in the order they were recieved by
    /// the process and may merge signals received multiple times.
    pub struct Signals(SignalIterator<UnixStream, UnixStream>);

    impl Signals {
        /// Create a `Signals` instance.
        ///
        /// This registers all the signals listed. The same restrictions (panics, errors) apply
        /// as with [`add_signal`][signal_hook::iterator::Signals::add_signal].
        pub fn new<I, S>(signals: I) -> Result<Self, Error>
        where
            I: IntoIterator<Item = S>,
            S: Borrow<c_int>,
        {
            let (read, write) = UnixStream::pair()?;
            let inner = SignalDelivery::with_pipe(read, write, signals)?;
            Ok(Self(SignalIterator::new(inner)))
        }

        /// Check if signals arrived by polling the stream for some bytes.
        ///
        /// Returns true if it was possible to read a byte and false otherwise.
        fn has_signals(read: &mut UnixStream) -> Result<bool, Error> {
            match read.poll_read(&mut [0u8]) {
                Poll::Ok(Async::NotReady) => Ok(false),
                Poll::Ok(Async::Ready(num_read)) => Ok(num_read > 0),
                Poll::Err(error) => Err(error),
            }
        }

        /// Get a shareable handle to a [`Controller`] for this instance.
        ///
        /// This can be used to add further signals or close the [`Signals`] instance
        /// which terminates the whole signal stream.
        pub fn controller(&self) -> ControllerHandle {
            self.0.controller()
        }
    }

    impl Stream for Signals {
        type Item = libc::c_int;
        type Error = Error;
        fn poll(&mut self) -> Poll<Option<libc::c_int>, Self::Error> {
            match self.0.poll_signal(&mut Signals::has_signals) {
                PollResult::Pending => Poll::Ok(Async::NotReady),
                PollResult::Signal(result) => Poll::Ok(Async::Ready(Some(result))),
                PollResult::Closed => Poll::Ok(Async::Ready(None)),
                PollResult::Err(error) => Poll::Err(error),
            }
        }
    }
}
