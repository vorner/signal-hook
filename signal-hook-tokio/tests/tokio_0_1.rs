#![cfg(feature = "support-v0_1")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use signal_hook_tokio::v0_1::Signals;
use tokio_0_1::prelude::*;
use tokio_0_1::timer::Interval;

use serial_test::serial;

fn send_sig(sig: libc::c_int) {
    unsafe { libc::raise(sig) };
}

#[test]
#[serial]
fn repeated() {
    let signals = Signals::new(&[signal_hook::SIGUSR1])
        .unwrap()
        .take(20)
        .map_err(|e| panic!("{}", e))
        .for_each(|sig| {
            assert_eq!(sig, signal_hook::SIGUSR1);
            send_sig(signal_hook::SIGUSR1);
            Ok(())
        });
    send_sig(signal_hook::SIGUSR1);
    tokio_0_1::run(signals);
}

/// A test where we actually wait for something ‒ the stream/reactor goes to sleep.
#[test]
#[serial]
fn delayed() {
    const CNT: usize = 10;
    let cnt = Arc::new(AtomicUsize::new(0));
    let inc_cnt = Arc::clone(&cnt);
    let signals = Signals::new(&[signal_hook::SIGUSR1, signal_hook::SIGUSR2])
        .unwrap()
        .filter(|sig| *sig == signal_hook::SIGUSR2)
        .take(CNT as u64)
        .map_err(|e| panic!("{}", e))
        .for_each(move |_| {
            inc_cnt.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
    let senders = Interval::new(Instant::now(), Duration::from_millis(250))
        .map_err(|e| panic!("{}", e))
        .for_each(|_| {
            send_sig(signal_hook::SIGUSR2);
            Ok(())
        });
    let both = signals.select(senders).map(|_| ()).map_err(|_| ());
    tokio_0_1::run(both);
    // Just make sure it didn't terminate prematurely
    assert_eq!(CNT, cnt.load(Ordering::Relaxed));
}

#[test]
#[serial]
fn close_signal_stream() {
    let mut signals = Signals::new(&[signal_hook::SIGUSR1, signal_hook::SIGUSR2]).unwrap();
    signals.handle().close();

    let async_result = signals.poll().unwrap();

    assert_eq!(async_result, Async::Ready(None));
}
