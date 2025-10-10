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
//! use async_std::prelude::*;
//!
//! use signal_hook::consts::signal::*;
//! use signal_hook_async_std::Signals;
//!
//! async fn handle_signals(mut signals: Signals) {
//!     while let Some(signal) = signals.next().await {
//!         match signal {
//!             SIGHUP => {
//!                 // Reload configuration
//!                 // Reopen the log file
//!             }
//!             SIGTERM | SIGINT | SIGQUIT => {
//!                 // Shutdown the system;
//!             },
//!             _ => unreachable!(),
//!         }
//!     }
//! }
//!
//! #[async_std::main]
//! async fn main() -> Result<(), Error> {
//!     let signals = Signals::new(&[SIGHUP, SIGTERM, SIGINT, SIGQUIT])?;
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
use std::os::unix::net::UnixStream;
use std::pin::Pin;
use std::task::{Context, Poll};

use libc::c_int;

pub use signal_hook::iterator::backend::Handle;
use signal_hook::iterator::backend::{OwningSignalIterator, PollResult, SignalDelivery};
use signal_hook::iterator::exfiltrator::{Exfiltrator, SignalOnly};

use async_io::Async;
use futures_lite::io::AsyncRead;
use futures_lite::stream::Stream;
use futures_lite::StreamExt;
use signal_hook::consts;

/// An asynchronous [`Stream`] of arriving signals.
///
/// The stream doesn't return the signals in the order they were recieved by
/// the process and may merge signals received multiple times.
pub struct SignalsInfo<E: Exfiltrator = SignalOnly>(OwningSignalIterator<Async<UnixStream>, E>);

impl<E: Exfiltrator> SignalsInfo<E> {
    /// Create a `Signals` instance.
    ///
    /// This registers all the signals listed. The same restrictions (panics, errors) apply
    /// as with [`Handle::add_signal`].
    pub fn new<I, S>(signals: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
        E: Default,
    {
        Self::with_exfiltrator(signals, E::default())
    }

    /// A constructor with explicit exfiltrator.
    pub fn with_exfiltrator<I, S>(signals: I, exfiltrator: E) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: Borrow<c_int>,
    {
        let (read, write) = Async::<UnixStream>::pair()?;
        let inner = SignalDelivery::with_pipe(read, write, exfiltrator, signals)?;
        Ok(Self(OwningSignalIterator::new(inner)))
    }

    /// Get a shareable [`Handle`] for this `Signals` instance.
    ///
    /// This can be used to add further signals or close the [`Signals`] instance
    /// which terminates the whole signal stream.
    pub fn handle(&self) -> Handle {
        self.0.handle()
    }
}

impl<E: Exfiltrator> SignalsInfo<E> {
    fn has_signals(read: &mut Async<UnixStream>, ctx: &mut Context<'_>) -> Result<bool, Error> {
        match Pin::new(read).poll_read(ctx, &mut [0u8]) {
            Poll::Pending => Ok(false),
            Poll::Ready(Ok(num_read)) => Ok(num_read > 0),
            Poll::Ready(Err(error)) => Err(error),
        }
    }
}

impl<E: Exfiltrator> Stream for SignalsInfo<E> {
    type Item = E::Output;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.0.poll_signal(&mut |read| Self::has_signals(read, ctx)) {
            PollResult::Signal(sig) => Poll::Ready(Some(sig)),
            PollResult::Closed => Poll::Ready(None),
            PollResult::Pending => Poll::Pending,
            PollResult::Err(error) => panic!("Unexpected error: {}", error),
        }
    }
}

/// Simplified version of the signals stream.
///
/// This one simply returns the signal numbers, while [`SignalsInfo`] can provide additional
/// information.
pub type Signals = SignalsInfo<SignalOnly>;

/// Waits for the the process to receive a shutdown signal.
/// Waits for any of SIGHUP, SIGINT, SIGQUIT, and SIGTERM.
/// # Errors
/// Returns `Err` after failing to register the signal handler.
pub async fn wait_for_shutdown_signal() -> Result<(), String> {
    let signals = [
        consts::SIGHUP,
        consts::SIGINT,
        consts::SIGQUIT,
        consts::SIGTERM,
    ];
    let mut signals = Signals::new(&signals)
        .map_err(|e| format!("error setting up handler for signals {signals:?}: {e}"))?;
    let _ = signals.next().await;
    Ok(())
}
