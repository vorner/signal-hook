#![cfg(feature = "futures-v0_3")]

use futures::stream::StreamExt;

use libc::SIGHUP;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use signal_hook::consts::SIGUSR1;
use signal_hook::low_level::raise;
use signal_hook_tokio::Signals;

use serial_test::serial;

#[tokio::test]
#[serial]
async fn next_returns_recieved_signal() {
    let mut signals = Signals::new(&[SIGUSR1]).unwrap();
    raise(SIGUSR1).unwrap();

    let signal = signals.next().await;

    assert_eq!(signal, Some(SIGUSR1));
}

#[tokio::test]
#[serial]
async fn close_signal_stream() {
    let mut signals = Signals::new(&[SIGUSR1]).unwrap();
    signals.handle().close();

    let result = signals.next().await;

    assert_eq!(result, None);
}

#[tokio::test]
#[serial]
async fn delayed() {
    async fn get_signal(mut signals: Signals, recieved: Arc<AtomicBool>) {
        signals.next().await;
        recieved.store(true, Ordering::SeqCst);
    }

    let signals = Signals::new(&[SIGUSR1]).unwrap();
    let recieved = Arc::new(AtomicBool::new(false));

    let signals_task = tokio::spawn(get_signal(signals, Arc::clone(&recieved)));

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!recieved.load(Ordering::SeqCst));

    raise(SIGUSR1).unwrap();
    signals_task.await.unwrap();
    assert!(recieved.load(Ordering::SeqCst));
}

#[tokio::test]
#[serial]
async fn wait_for_shutdown_signal() {
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let elapsed_ms_clone = Arc::clone(&elapsed_ms);
    tokio::spawn(async move {
        let before = Instant::now();
        signal_hook_tokio::wait_for_shutdown_signal().await.unwrap();
        let elapsed_ms_u64 =
            u64::try_from(Instant::now().saturating_duration_since(before).as_millis())
                .unwrap_or(u64::MAX);
        elapsed_ms_clone.store(elapsed_ms_u64, Ordering::Release)
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    raise(SIGHUP).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let elapsed_ms_u64 = elapsed_ms.load(Ordering::Acquire);
    assert!((50..=150).contains(&elapsed_ms_u64), "{:?}", elapsed_ms_u64);
}
