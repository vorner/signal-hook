use std::io::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use libc::c_int;

use SigId;

pub fn register_flag(signal: c_int, flag: Arc<AtomicBool>) -> Result<SigId, Error> {
    unsafe { ::register_action(signal, move || flag.store(true, Ordering::Relaxed)) }
}

pub fn register_usize_flag(
    signal: c_int,
    flag: Arc<AtomicUsize>,
    value: usize,
) -> Result<SigId, Error> {
    unsafe { ::register_action(signal, move || flag.store(value, Ordering::Relaxed)) }
}

pub const SIGNUM: usize = 32;

pub type Registry = [AtomicBool; SIGNUM];

pub fn register_registry<I>(signals: I, registry: &Arc<Registry>) -> Result<Vec<SigId>, Error>
where
    I: IntoIterator<Item = c_int>,
{
    signals
        .into_iter()
        .map(|sig| {
            assert!(sig < registry.len() as c_int);
            assert!(sig >= 0);
            let registry = Arc::clone(&registry);
            unsafe {
                ::register_action(sig, move || {
                    registry[sig as usize].store(true, Ordering::Relaxed)
                })
            }
        })
        .collect()
}
