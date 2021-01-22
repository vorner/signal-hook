#![doc(test(attr(deny(warnings))))]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "futures-v0_3"), allow(dead_code, unused_imports))]

//! A crate for integrating signal handling with the Tokio runtime.
//!
//! This provides the [`Signals`][Signals] struct which acts as a
//! [`Stream`][futures_core_0_3::stream::Stream] of signals.
//!
//! Note that the `futures-v0_3` feature of this crate must be
//! enabled for `Signals` to implement the `Stream` trait.
//!
//! # Example
//!
//! ```rust
//! use std::io::Error;
//!
//! use signal_hook::consts::signal::*;
//! use signal_hook_tokio::Signals;
//!
//! use futures::stream::StreamExt;
//!
//! async fn handle_signals(signals: Signals) {
//!     let mut signals = signals.fuse();
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
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     let signals = Signals::new(&[
//!         SIGHUP,
//!         SIGTERM,
//!         SIGINT,
//!         SIGQUIT,
//!     ])?;
//!     let handle = signals.handle();
//!
//!     let signals_task = tokio::spawn(handle_signals(signals));
//!
//!     // Execute your main program logic
//!
//!     // Terminate the signal stream.
//!     handle.close();
//!     signals_task.await?;
//!
//!     Ok(())
//! }
//! ```

macro_rules! implement_signals_with_pipe {
    ($pipe:ty) => {
        use std::borrow::Borrow;
        use std::io::Error;

        use libc::c_int;

        pub use signal_hook::iterator::backend::Handle;
        use signal_hook::iterator::backend::{OwningSignalIterator, SignalDelivery};
        use signal_hook::iterator::exfiltrator::{Exfiltrator, SignalOnly};

        /// An asynchronous [`Stream`] of arriving signals.
        ///
        /// The stream doesn't return the signals in the order they were recieved by
        /// the process and may merge signals received multiple times.
        pub struct SignalsInfo<E: Exfiltrator = SignalOnly>(OwningSignalIterator<$pipe, E>);

        impl<E: Exfiltrator> SignalsInfo<E> {
            /// Create a `SignalsInfo` instance.
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
                let (read, write) = UnixStream::pair()?;
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

        /// Simplified version of the signals stream.
        ///
        /// This one simply returns the signal numbers, while [`SignalsInfo`] can provide additional
        /// information.
        pub type Signals = SignalsInfo<SignalOnly>;
    };
}

use tokio::net::UnixStream;
implement_signals_with_pipe!(UnixStream);

use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(feature = "futures-v0_3")]
use futures_core_0_3::Stream;

use signal_hook::iterator::backend::PollResult;
use tokio::io::{AsyncRead, ReadBuf};

impl Signals {
    fn has_signals(read: &mut UnixStream, ctx: &mut Context<'_>) -> Result<bool, Error> {
        let mut buf = [0u8];
        let mut read_buf = ReadBuf::new(&mut buf);
        match Pin::new(read).poll_read(ctx, &mut read_buf) {
            Poll::Pending => Ok(false),
            Poll::Ready(Ok(())) => Ok(true),
            Poll::Ready(Err(error)) => Err(error),
        }
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "futures-v0_3")))]
#[cfg(feature = "futures-v0_3")]
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
