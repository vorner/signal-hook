#![doc(test(attr(deny(warnings))))]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A crate for integrating signal handling with the Tokio runtime.
//!
//! There are different sub modules for supporting different Tokio
//! versions. The support for a version must be activated by a
//! feature flag:
//!
//! * `support-v0_1` for sub module [`v0_1`]
//! * `support-v0_3` for sub module [`v0_3`]
//!
//! See the specific sub modules for usage examples.

#[cfg(any(feature = "support-v0_1", feature = "support-v0_3"))]
macro_rules! implement_signals_with_pipe {
    ($pipe:ty) => {
        use std::borrow::Borrow;
        use std::io::Error;

        use libc::c_int;

        pub use signal_hook::iterator::backend::Handle;
        use signal_hook::iterator::backend::{SignalDelivery, SignalIterator};
        use signal_hook::iterator::exfiltrator::{Exfiltrator, SignalOnly};

        /// An asynchronous [`Stream`] of arriving signals.
        ///
        /// The stream doesn't return the signals in the order they were recieved by
        /// the process and may merge signals received multiple times.
        pub struct SignalsInfo<E: Exfiltrator = SignalOnly>(SignalIterator<$pipe, E>);

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

        /// Simplified version of the signals stream.
        ///
        /// This one simply returns the signal numbers, while [`SignalsInfo`] can provide additional
        /// information.
        pub type Signals = SignalsInfo<SignalOnly>;
    };
}

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
    use futures_0_1::stream::Stream;
    use futures_0_1::{Async, Poll};
    use signal_hook::iterator::backend::PollResult;
    use tokio_0_1::io::AsyncRead;
    use tokio_0_1::net::unix::UnixStream;

    implement_signals_with_pipe!(UnixStream);

    impl<E: Exfiltrator> SignalsInfo<E> {
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
    }

    impl<E: Exfiltrator> Stream for SignalsInfo<E> {
        type Item = E::Output;
        type Error = Error;
        fn poll(&mut self) -> Poll<Option<E::Output>, Self::Error> {
            match self.0.poll_signal(&mut SignalsInfo::<E>::has_signals) {
                PollResult::Pending => Poll::Ok(Async::NotReady),
                PollResult::Signal(result) => Poll::Ok(Async::Ready(Some(result))),
                PollResult::Closed => Poll::Ok(Async::Ready(None)),
                PollResult::Err(error) => Poll::Err(error),
            }
        }
    }
}

/// A module for integrating signal handling with the Tokio 0.3 runtime.
///
/// This provides the [`Signals`][v0_3::Signals] struct which acts as a
/// [`Stream`][`futures_0_3::stream::Stream`] of signals.
///
/// # Example
///
/// ```rust
/// # use tokio_0_3 as tokio;
/// # use futures_0_3 as futures;
///
/// use std::io::Error;
///
/// use signal_hook;
/// use signal_hook_tokio::v0_3::Signals;
///
/// use futures::stream::StreamExt;
///
/// async fn handle_signals(signals: Signals) {
///     let mut signals = signals.fuse();
///     while let Some(signal) = signals.next().await {
///         match signal {
///             signal_hook::SIGHUP => {
///                 // Reload configuration
///                 // Reopen the log file
///             }
///             signal_hook::SIGTERM | signal_hook::SIGINT | signal_hook::SIGQUIT => {
///                 // Shutdown the system;
///             },
///             _ => unreachable!(),
///         }
///     }
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let signals = Signals::new(&[
///         signal_hook::SIGHUP,
///         signal_hook::SIGTERM,
///         signal_hook::SIGINT,
///         signal_hook::SIGQUIT,
///     ])?;
///     let handle = signals.handle();
///
///     let signals_task = tokio::spawn(handle_signals(signals));
///
///     // Execute your main program logic
///
///     // Terminate the signal stream.
///     handle.close();
///     signals_task.await?;
///
///     Ok(())
/// }
/// ```
#[cfg(feature = "support-v0_3")]
pub mod v0_3 {
    use std::pin::Pin;

    use futures_0_3::stream::Stream;
    use futures_0_3::task::{Context, Poll};
    use signal_hook::iterator::backend::PollResult;
    use tokio_0_3::io::{AsyncRead, ReadBuf};
    use tokio_0_3::net::UnixStream;

    implement_signals_with_pipe!(UnixStream);

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
}
