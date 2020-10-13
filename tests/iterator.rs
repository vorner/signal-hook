#![cfg(not(windows))]

extern crate signal_hook;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use signal_hook::iterator::Signals;
use signal_hook::{SIGUSR1, SIGUSR2};

use serial_test::serial;

fn send_sigusr1() {
    unsafe { libc::raise(SIGUSR1) };
}

fn send_sigusr2() {
    unsafe { libc::raise(SIGUSR2) };
}

fn setup_without_any_signals() -> Signals {
    Signals::new(&[]).unwrap()
}

fn setup_for_sigusr2() -> Signals {
    Signals::new(&[SIGUSR2]).unwrap()
}

macro_rules! assert_signals {
    ($actual:expr, $($expected:expr),+ $(,)?) => {
        let actual = $actual.collect::<HashSet<libc::c_int>>();
        let expected = vec!($($expected),+).into_iter().collect::<HashSet<libc::c_int>>();
        assert_eq!(actual, expected);
    };
}

macro_rules! assert_no_signals {
    ($signals:expr) => {
        assert_eq!($signals.next(), None);
    };
}

#[test]
#[serial]
fn signals_close_forever() {
    // The cloned instances are connected to each other. Closing one closes all.
    // Closing it terminates the forever that waits for stuff. Well, it terminates all of them.
    let signals = Signals::new(&[SIGUSR1]).unwrap();
    // Detect early terminations.
    let stopped = Arc::new(AtomicBool::new(false));
    let threads = (0..5).map(|_| {
        let signals_bg = signals.clone();
        let stopped_bg = Arc::clone(&stopped);
        thread::spawn(move || {
            // Eat all the signals there are (might come from a concurrent test, in theory).
            // Would wait forever, but it should be terminated by the close below.
            for _sig in &signals_bg {}

            stopped_bg.store(true, Ordering::SeqCst);
        })
    });

    // The map method is lazy and will execute the function only during iteration. So we
    // collect the JoinHandles to ensure the threads are actually started.
    let threads: Vec<thread::JoinHandle<()>> = threads.collect();

    // Wait a bit… if some thread terminates by itself.
    thread::sleep(Duration::from_millis(100));
    assert!(!stopped.load(Ordering::SeqCst));

    signals.close();

    // If they don't terminate correctly, the test just keeps running. Not the best way to do
    // tests, but whatever…
    for thread in threads {
        thread.join().unwrap();
    }
}

// A reproducer for #16: if we had the mio-support enabled (which is enabled also by the
// tokio-support feature), blocking no longer works. The .wait() would return immediately (an empty
// iterator, possibly), .forever() would do a busy loop.
// flag)
#[test]
#[serial]
fn signals_block_wait() {
    let signals = Signals::new(&[SIGUSR2]).unwrap();
    let (s, r) = mpsc::channel();
    thread::spawn(move || {
        // Technically, it may spuriously return early. But it shouldn't be doing it too much, so
        // we just try to wait multiple times ‒ if they *all* return right away, it is broken.
        for _ in 0..10 {
            for _ in signals.wait() {
                panic!("Someone really did send us SIGUSR2, which breaks the test");
            }
        }
        let _ = s.send(());
    });

    let err = r
        .recv_timeout(Duration::from_millis(100))
        .expect_err("Wait didn't wait properly");
    assert_eq!(err, RecvTimeoutError::Timeout);
}

#[test]
#[serial]
fn pending_doesnt_block() {
    let signals = setup_for_sigusr2();

    let mut recieved_signals = signals.pending();

    assert_no_signals!(recieved_signals);
}

#[test]
#[serial]
fn wait_returns_recieved_signals() {
    let signals = setup_for_sigusr2();
    send_sigusr2();

    let recieved_signals = signals.wait();

    assert_signals!(recieved_signals, SIGUSR2);
}

#[test]
#[serial]
fn forever_returns_recieved_signals() {
    let signals = setup_for_sigusr2();
    send_sigusr2();

    let signal = signals.forever().take(1);

    assert_signals!(signal, SIGUSR2);
}

#[test]
#[serial]
fn wait_doesnt_block_when_closed() {
    let signals = setup_for_sigusr2();
    signals.close();

    let mut recieved_signals = signals.wait();

    assert_no_signals!(recieved_signals);
}

#[test]
#[serial]
fn wait_unblocks_when_closed() {
    let signals = setup_without_any_signals();
    let close_handle = signals.clone();

    let thread = thread::spawn(move || {
        signals.wait();
    });

    close_handle.close();

    thread.join().unwrap();
}

#[test]
#[serial]
fn forever_doesnt_block_when_closed() {
    let signals = setup_for_sigusr2();
    signals.close();

    let mut signal = signals.forever();

    assert_no_signals!(signal);
}

#[test]
#[serial]
fn add_signal_after_creation() {
    let signals = setup_without_any_signals();
    signals.add_signal(SIGUSR1).unwrap();

    send_sigusr1();

    assert_signals!(signals.pending(), SIGUSR1);
}

#[test]
#[serial]
fn delayed_signal_consumed() {
    let signals = setup_for_sigusr2();
    signals.add_signal(SIGUSR1).unwrap();

    send_sigusr1();
    let mut recieved_signals = signals.wait();
    send_sigusr2();

    assert_signals!(recieved_signals, SIGUSR1, SIGUSR2);

    // The pipe still contains the byte from the second
    // signal and so wait won't block but won't return
    // a signal.
    recieved_signals = signals.wait();
    assert_no_signals!(recieved_signals);
}

#[test]
#[serial]
fn is_closed_initially_returns_false() {
    let signals = setup_for_sigusr2();

    assert_eq!(signals.is_closed(), false);
}

#[test]
#[serial]
fn is_closed_returns_true_when_closed() {
    let signals = setup_for_sigusr2();
    signals.close();

    assert_eq!(signals.is_closed(), true);
}
