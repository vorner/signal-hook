#![doc(test(attr(deny(warnings))))]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A module for integrating signal handling with the async-std runtime.
//!
//! This provides the [`Signals`] struct which acts as a
//! [`Stream`] of signals.
//!
//! # Example
//!
//! ```rust
//! use std::io::Error;
//!
//! use async_std;
//!
//! use futures::stream::StreamExt;
//!
//! use signal_hook;
//! use signal_hook_async_std::Signals;
//!
//! async fn handle_signals(signals: Signals) {
//!     let mut signals = signals.fuse();
//!     while let Some(signal) = signals.next().await {
//!         match signal {
//!             signal_hook::SIGHUP => {
//!                 // Reload configuration
//!                 // Reopen the log file
//!             }
//!             signal_hook::SIGTERM | signal_hook::SIGINT | signal_hook::SIGQUIT => {
//!                 // Shutdown the system;
//!             },
//!             _ => unreachable!(),
//!         }
//!     }
//! }
//!
//! #[async_std::main]
//! async fn main() -> Result<(), Error> {
//!     let signals = Signals::new(&[
//!         signal_hook::SIGHUP,
//!         signal_hook::SIGTERM,
//!         signal_hook::SIGINT,
//!         signal_hook::SIGQUIT,
//!     ])?;
//!     let handle = signals.handle();
//!
//!     let signals_task = async_std::task::spawn(handle_signals(signals));
//!
//!     // Execute your main program logic
//!
//!     // Terminate the signal stream.
//!     handle.close();
//!     signals_task.await;
//!
//!     Ok(())
//! }
//! ```

use std::borrow::Borrow;
use std::io::Error;
use std::pin::Pin;

use libc::c_int;

pub use signal_hook::iterator::backend::Handle;
use signal_hook::iterator::backend::{PollResult, SignalDelivery, SignalIterator};

use async_std::os::unix::net::UnixStream;

use futures::stream::Stream;
use futures::task::{Context, Poll};
use futures::AsyncRead;

/// An asynchronous [`Stream`] of arriving signals.
///
/// The stream doesn't return the signals in the order they were recieved by
/// the process and may merge signals received multiple times.
pub struct Signals(SignalIterator<UnixStream>);

impl Signals {
    /// Create a `Signals` instance.
    ///
    /// This registers all the signals listed. The same restrictions (panics, errors) apply
    /// as with [`Handle::add_signal`].
    pub fn new<I, S>(signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
    {
        let (read, write) = UnixStream::pair()?;
        let inner = SignalDelivery::with_pipe(read, write, signals)?;
        Ok(Self(SignalIterator::new(inner)))
    }

    /// Get a shareable [`Handle`] for this `Signals` instance.
    ///
    /// This can be used to add further signals or close the [`Signals`] instance
    /// which terminates the whole signal stream.
    pub fn handle(&self) -> Handle {
        self.0.handle()
    }
}

impl Signals {
    fn has_signals(read: &mut UnixStream, ctx: &mut Context<'_>) -> Result<bool, Error> {
        match Pin::new(read).poll_read(ctx, &mut [0u8]) {
            Poll::Pending => Ok(false),
            Poll::Ready(Ok(num_read)) => Ok(num_read > 0),
            Poll::Ready(Err(error)) => Err(error),
        }
    }
}

impl Stream for Signals {
    type Item = c_int;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.0.poll_signal(&mut |read| Self::has_signals(read, ctx)) {
            PollResult::Signal(sig) => Poll::Ready(Some(sig)),
            PollResult::Closed => Poll::Ready(None),
            PollResult::Pending => Poll::Pending,
            PollResult::Err(error) => panic!("Unexpected error: {}", error),
        }
    }
}
