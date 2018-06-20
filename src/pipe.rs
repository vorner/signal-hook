use std::io::Error;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use libc::{self, c_int};

use flag::Registry;
use SigId;

fn wake(pipe: RawFd) {
    unsafe {
        libc::send(pipe, b"X" as *const _ as *const _, 1, libc::MSG_DONTWAIT);
    }
}

pub fn self_pipe_raw(signal: c_int, pipe: RawFd) -> Result<SigId, Error> {
    unsafe { ::register_action(signal, move || wake(pipe)) }
}

pub fn self_pipe<P>(signal: c_int, pipe: P) -> Result<SigId, Error>
where
    P: AsRawFd + Send + Sync + 'static,
{
    let action = move || wake(pipe.as_raw_fd());
    unsafe { ::register_action(signal, action) }
}

pub struct Waker {
    pub read: UnixStream,
    write: UnixStream,
    pub registry: Registry,
}

pub fn waker<I>(signals: I) -> Result<(Vec<SigId>, Arc<Waker>), Error>
where
    I: IntoIterator<Item = c_int>,
{
    let (read, write) = UnixStream::pair()?;
    let registry = Registry::default();
    let waker = Arc::new(Waker {
        read,
        write,
        registry,
    });
    let ids = signals
        .into_iter()
        .map(|sig| {
            assert!(sig >= 0);
            assert!(sig < waker.registry.len() as c_int);
            let waker = Arc::clone(&waker);
            let action = move || {
                waker.registry[sig as usize].store(true, Ordering::Relaxed);
                wake(waker.write.as_raw_fd());
            };
            unsafe { ::register_action(sig, action) }
        })
        .collect::<Result<_, _>>()?;
    Ok((ids, waker))
}
