use std::time::{Duration, Instant};

#[test]
fn wait_for_shutdown_signal() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let before = Instant::now();
        signal_hook::wait_for_shutdown_signal().unwrap();
        let elapsed = Instant::now().saturating_duration_since(before);
        sender.send(elapsed).unwrap();
    });
    std::thread::sleep(Duration::from_millis(100));
    signal_hook::low_level::raise(signal_hook::consts::SIGHUP).unwrap();
    let elapsed = receiver.recv_timeout(Duration::from_millis(100)).unwrap();
    assert!(
        (Duration::from_millis(50)..=Duration::from_millis(150)).contains(&elapsed),
        "{:?}",
        elapsed
    );
}
