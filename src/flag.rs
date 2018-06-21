//! Module for actions setting flags.
//!
//! This contains helper functions to set flags whenever a signal happens.

use std::io::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use libc::c_int;

use SigId;

/// Registers an action to set the flag to `true` whenever the given signal arrives.
pub fn register_flag(signal: c_int, flag: Arc<AtomicBool>) -> Result<SigId, Error> {
    unsafe { ::register(signal, move || flag.store(true, Ordering::Relaxed)) }
}

/// Registers an action to set the flag to the given value whenever the signal arrives.
pub fn register_usize_flag(
    signal: c_int,
    flag: Arc<AtomicUsize>,
    value: usize,
) -> Result<SigId, Error> {
    unsafe { ::register(signal, move || flag.store(value, Ordering::Relaxed)) }
}
