extern crate lazy_static;

use std::sync::{Mutex, MutexGuard};

use lazy_static::lazy_static;

lazy_static! {
    static ref LOCK: Mutex<()> = Mutex::new(());
}

pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap()
}
