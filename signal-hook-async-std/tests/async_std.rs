use async_std::stream::StreamExt;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use signal_hook::consts::SIGUSR1;
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
