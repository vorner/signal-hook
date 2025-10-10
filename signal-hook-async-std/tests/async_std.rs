use async_std::stream::StreamExt;
use std::convert::TryFrom;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGHUP, SIGUSR1};
use signal_hook::low_level::raise;
use signal_hook_async_std::Signals;

use serial_test::serial;

#[async_std::test]
#[serial]
async fn next_returns_recieved_signal() {
    let mut signals = Signals::new(&[SIGUSR1]).unwrap();
    raise(SIGUSR1).unwrap();

    let signal = signals.next().await;

    assert_eq!(signal, Some(SIGUSR1));
}

#[async_std::test]
#[serial]
async fn close_signal_stream() {
    let mut signals = Signals::new(&[SIGUSR1]).unwrap();
    signals.handle().close();

    let result = signals.next().await;

    assert_eq!(result, None);
}

#[async_std::test]
#[serial]
async fn delayed() {
    async fn get_signal(mut signals: Signals, recieved: Arc<AtomicBool>) {
        signals.next().await;
        recieved.store(true, Ordering::SeqCst);
    }

    let signals = Signals::new(&[SIGUSR1]).unwrap();
    let recieved = Arc::new(AtomicBool::new(false));

    let signals_task = async_std::task::spawn(get_signal(signals, Arc::clone(&recieved)));

    async_std::task::sleep(Duration::from_millis(100)).await;
    assert!(!recieved.load(Ordering::SeqCst));

    raise(SIGUSR1).unwrap();
    signals_task.await;
    assert!(recieved.load(Ordering::SeqCst));
}

#[async_std::test]
#[serial]
async fn wait_for_shutdown_signal() {
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let elapsed_ms_clone = Arc::clone(&elapsed_ms);
    async_std::task::spawn(async move {
        let before = Instant::now();
        signal_hook_async_std::wait_for_shutdown_signal()
            .await
            .unwrap();
        let elapsed_ms_u64 =
            u64::try_from(Instant::now().saturating_duration_since(before).as_millis())
                .unwrap_or(u64::MAX);
        elapsed_ms_clone.store(elapsed_ms_u64, Ordering::Release)
    });
    async_std::task::sleep(Duration::from_millis(100)).await;
    raise(SIGHUP).unwrap();
    async_std::task::sleep(Duration::from_millis(100)).await;
    let elapsed_ms_u64 = elapsed_ms.load(Ordering::Acquire);
    assert!((50..=150).contains(&elapsed_ms_u64), "{:?}", elapsed_ms_u64);
}
