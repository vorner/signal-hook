#![cfg(not(windows))]

#[cfg(feature = "tokio-support")]
mod tests {
    extern crate futures;
    extern crate libc;
    extern crate signal_hook;
    extern crate tokio;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use self::signal_hook::iterator::Signals;
    use self::tokio::prelude::*;
    use self::tokio::runtime::current_thread::Runtime;
    use self::tokio::timer::Interval;

    use self::futures::prelude::*;

    fn send_sig(sig: libc::c_int) {
        unsafe { libc::raise(sig) };
    }

    #[test]
    fn repeated() {
        let mut runtime = Runtime::new().unwrap();

        let signals = Signals::new(&[signal_hook::SIGUSR1])
            .unwrap()
            .into_async()
            .unwrap()
            .take(20)
            .map(|r| r.unwrap())
            .map(|sig| {
                assert_eq!(sig, signal_hook::SIGUSR1);
                send_sig(signal_hook::SIGUSR1);
            })
            .collect::<()>();
        send_sig(signal_hook::SIGUSR1);

        runtime.block_on(signals);
    }

    /// A test where we actually wait for something ‒ the stream/reactor goes to sleep.
    #[test]
    fn delayed() {
        let mut runtime = Runtime::new().unwrap();

        const CNT: usize = 10;
        let cnt = Arc::new(AtomicUsize::new(0));
        let inc_cnt = Arc::clone(&cnt);
        let signals = Signals::new(&[signal_hook::SIGUSR1, signal_hook::SIGUSR2])
            .unwrap()
            .into_async()
            .unwrap()
            .map(|r| r.unwrap())
            .filter(|sig| future::ready(*sig == signal_hook::SIGUSR2))
            .take(CNT as u64)
            .map(move |_| {
                inc_cnt.fetch_add(1, Ordering::Relaxed);
            });

        let senders = Interval::new(Instant::now(), Duration::from_millis(250))
            .map(|_| send_sig(signal_hook::SIGUSR2));

        let both = signals.zip(senders).map(drop).collect::<()>();

        runtime.block_on(both);

        // Just make sure it didn't terminate prematurely
        assert_eq!(CNT, cnt.load(Ordering::Relaxed));
    }
}
