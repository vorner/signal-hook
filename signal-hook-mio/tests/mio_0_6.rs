#![cfg(feature = "support-v0_6")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use mio_0_6::{Events, Poll, PollOpt, Ready, Token};
use signal_hook_mio::v0_6::Signals;

use serial_test::serial;

use libc::c_int;

#[test]
#[serial]
fn mio_wakeup() {
    let signals = Signals::new(&[signal_hook::SIGUSR1]).unwrap();
    let token = Token(0);
    let poll = Poll::new().unwrap();
    poll.register(&signals, token, Ready::readable(), PollOpt::level())
        .unwrap();
    let mut events = Events::with_capacity(10);

    // The self pipe shouldn't be readable yet
    poll.poll(&mut events, Some(Duration::from_secs(0)))
        .unwrap();
    assert!(events.is_empty());

    unsafe { libc::raise(signal_hook::SIGUSR1) };
    poll.poll(&mut events, Some(Duration::from_secs(10)))
        .unwrap();
    let event = events.iter().next().unwrap();
    assert!(event.readiness().is_readable());
    assert_eq!(token, event.token());
    let sig = signals.pending().next().unwrap();
    assert_eq!(signal_hook::SIGUSR1, sig);

    // The self pipe shouldn't be readable after consuming signals
    poll.poll(&mut events, Some(Duration::from_secs(0)))
        .unwrap();
    assert!(events.is_empty());
}

#[test]
#[serial]
fn mio_multiple_signals() {
    let signals = Signals::new(&[signal_hook::SIGUSR1, signal_hook::SIGUSR2]).unwrap();
    let poll = Poll::new().unwrap();
    let token = Token(0);
    poll.register(&signals, token, Ready::readable(), PollOpt::level())
        .unwrap();

    let mut events = Events::with_capacity(10);

    unsafe {
        libc::raise(signal_hook::SIGUSR1);
        libc::raise(signal_hook::SIGUSR2);
    };

    poll.poll(&mut events, Some(Duration::from_secs(10)))
        .unwrap();

    let event = events.iter().next().unwrap();
    assert!(event.readiness().is_readable());

    let sigs: Vec<c_int> = signals.pending().collect();
    assert_eq!(2, sigs.len());
    assert!(sigs.contains(&signal_hook::SIGUSR1));
    assert!(sigs.contains(&signal_hook::SIGUSR2));

    // The self pipe shouldn't be completely empty after calling pending()
    poll.poll(&mut events, Some(Duration::from_secs(0)))
        .unwrap();
    assert!(events.is_empty());
}

#[test]
#[serial]
fn mio_parallel_multiple() {
    let signals = Signals::new(&[signal_hook::SIGUSR1]).unwrap();
    let poll = Poll::new().unwrap();
    let token = Token(0);
    poll.register(&signals, token, Ready::readable(), PollOpt::level())
        .unwrap();

    let mut events = Events::with_capacity(10);

    let thread_done = Arc::new(AtomicBool::new(false));

    let done = Arc::clone(&thread_done);
    thread::spawn(move || {
        for _ in 0..10 {
            // Wait some time to allow main thread to poll
            thread::sleep(Duration::from_millis(25));
            unsafe { libc::raise(signal_hook::SIGUSR1) };
        }
        done.store(true, Ordering::SeqCst);

        // Raise a final signal so the main thread wakes up
        // if it already called poll.
        unsafe { libc::raise(signal_hook::SIGUSR1) };
    });

    while !thread_done.load(Ordering::SeqCst) {
        poll.poll(&mut events, Some(Duration::from_secs(10)))
            .unwrap();
        let event = events.iter().next().unwrap();
        assert!(event.readiness().is_readable());
        assert_eq!(signal_hook::SIGUSR1, signals.pending().next().unwrap());
    }
}
