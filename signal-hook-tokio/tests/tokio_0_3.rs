#![cfg(feature = "support-v0_3")]

use futures_0_3 as futures;
use tokio_0_3 as tokio;

use futures::stream::StreamExt;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use signal_hook::consts::SIGUSR1;
use signal_hook::low_level::raise;
use signal_hook_tokio::v0_3::Signals;

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
    assert_eq!(recieved.load(Ordering::SeqCst), false);

    raise(SIGUSR1).unwrap();
    signals_task.await.unwrap();
    assert_eq!(recieved.load(Ordering::SeqCst), true);
}
