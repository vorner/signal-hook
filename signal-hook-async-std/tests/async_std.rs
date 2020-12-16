use async_std::stream::StreamExt;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use signal_hook::consts::SIGUSR1;
use signal_hook_async_std::Signals;

use serial_test::serial;

fn send_sig(sig: libc::c_int) {
    unsafe { libc::raise(sig) };
}

#[async_std::test]
#[serial]
async fn next_returns_recieved_signal() {
    let mut signals = Signals::new(&[SIGUSR1]).unwrap();
    send_sig(SIGUSR1);

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
    assert_eq!(recieved.load(Ordering::SeqCst), false);

    send_sig(SIGUSR1);
    signals_task.await;
    assert_eq!(recieved.load(Ordering::SeqCst), true);
}
