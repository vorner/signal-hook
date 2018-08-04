#[cfg(feature = "tokio-support")]
mod tests {
    extern crate libc;
    extern crate signal_hook;
    extern crate tokio;

    use self::signal_hook::iterator::Signals;
    use self::tokio::prelude::*;

    fn send_sig() {
        unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
    }

    #[test]
    fn repeated() {
        let signals = Signals::new(&[libc::SIGUSR1])
            .unwrap()
            .async()
            .unwrap()
            .map(|sig| {
                assert_eq!(sig, libc::SIGUSR1);
                send_sig();
            })
            .map_err(|e| panic!("{}", e))
            .take(20)
            .for_each(|()| Ok(()));
        send_sig();
        tokio::run(signals);
    }
}
