extern crate arc_swap;
extern crate libc;

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::io::Error;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::process;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, Once, ONCE_INIT};
use std::thread;

use arc_swap::ArcSwap;
use libc::{c_int, c_void, sigaction, siginfo_t, sigset_t, SIG_BLOCK, SIG_SETMASK};

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct ActionId(u64);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SigId {
    signal: c_int,
    action: ActionId,
}

#[derive(Clone)]
struct Slot {
    prev: sigaction,
    actions: BTreeMap<ActionId, Arc<Fn()>>,
}

impl Slot {
    fn new(signal: libc::c_int) -> Result<Self, Error> {
        // C data structure, expected to be zeroed out.
        let mut new: libc::sigaction = unsafe { mem::zeroed() };
        new.sa_sigaction = handler as usize;
        #[cfg(target_os = "android")]
        fn flags() -> libc::c_ulong {
            (libc::SA_RESTART as libc::c_ulong)
                | libc::SA_SIGINFO
                | (libc::SA_NOCLDSTOP as libc::c_ulong)
        }
        #[cfg(not(target_os = "android"))]
        fn flags() -> libc::c_int {
            libc::SA_RESTART | libc::SA_SIGINFO | libc::SA_NOCLDSTOP
        }
        new.sa_flags = flags();
        // C data structure, expected to be zeroed out.
        let mut old: libc::sigaction = unsafe { mem::zeroed() };
        // FFI ‒ pointers are valid, it doesn't take ownership.
        if unsafe { libc::sigaction(signal, &new, &mut old) } != 0 {
            return Err(Error::last_os_error());
        }
        Ok(Slot {
            prev: old,
            actions: BTreeMap::new(),
        })
    }
}

type AllSignals = HashMap<c_int, Slot>;

struct GlobalData {
    all_signals: ArcSwap<AllSignals>,
    rcu_lock: Mutex<u64>,
}

static mut GLOBAL_DATA: Option<GlobalData> = None;
static GLOBAL_INIT: Once = ONCE_INIT;

impl GlobalData {
    fn get() -> &'static Self {
        unsafe { GLOBAL_DATA.as_ref().unwrap() }
    }
    fn ensure() -> &'static Self {
        GLOBAL_INIT.call_once(|| unsafe {
            GLOBAL_DATA = Some(GlobalData {
                all_signals: ArcSwap::from(Arc::new(HashMap::new())),
                rcu_lock: Mutex::new(0),
            });
        });
        Self::get()
    }
    fn load(&self) -> (AllSignals, MutexGuard<u64>) {
        let lock = self.rcu_lock.lock().unwrap();
        let signals = AllSignals::clone(&self.all_signals.load());
        (signals, lock)
    }
    fn store(&self, signals: AllSignals, _lock: MutexGuard<u64>) {
        let mut previous = self.all_signals.swap(Arc::new(signals));
        while let Err(failed_unwrap) = Arc::try_unwrap(previous) {
            previous = failed_unwrap;
            thread::yield_now();
        }
    }
}

extern "C" fn handler(sig: c_int, info: *mut siginfo_t, data: *mut c_void) {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let signals = GlobalData::get().all_signals.load();

        if let Some(ref slot) = signals.get(&sig) {
            let fptr = slot.prev.sa_sigaction;
            if fptr != 0 && fptr != libc::SIG_DFL && fptr != libc::SIG_IGN {
                // FFI ‒ calling the original signal handler.
                unsafe {
                    if slot.prev.sa_flags & libc::SA_SIGINFO == 0 {
                        let action = mem::transmute::<usize, extern "C" fn(c_int)>(fptr);
                        action(sig);
                    } else {
                        type SigAction = extern "C" fn(c_int, *mut siginfo_t, *mut c_void);
                        let action = mem::transmute::<usize, SigAction>(fptr);
                        action(sig, info, data);
                    }
                }
            }

            for action in slot.actions.values() {
                action();
            }
        }
    }));
    if result.is_err() {
        eprintln!("Panic inside a signal handler for {}", sig);
        process::abort();
    }
}

fn block_signal(signal: c_int) -> Result<sigset_t, Error> {
    unsafe {
        let mut newsigs: sigset_t = mem::uninitialized();
        libc::sigemptyset(&mut newsigs);
        libc::sigaddset(&mut newsigs, signal);
        let mut oldsigs: sigset_t = mem::uninitialized();
        libc::sigemptyset(&mut oldsigs);
        if libc::sigprocmask(SIG_BLOCK, &newsigs, &mut oldsigs) == 0 {
            Ok(oldsigs)
        } else {
            Err(Error::last_os_error())
        }
    }
}

fn restore_signals(signals: libc::sigset_t) -> Result<(), Error> {
    if unsafe { libc::sigprocmask(SIG_SETMASK, &signals, ptr::null_mut()) } == 0 {
        Ok(())
    } else {
        Err(Error::last_os_error())
    }
}

fn without_signal<F: FnOnce() -> Result<(), Error>>(signal: c_int, f: F) -> Result<(), Error> {
    let old_signals = block_signal(signal)?;
    let result = f();
    let restored = restore_signals(old_signals);
    // In case of errors in both, prefer the one in result.
    result.and(restored)
}

pub unsafe fn register_action(signal: c_int, action: Box<Fn()>) -> Result<SigId, Error> {
    let globals = GlobalData::ensure();
    let (mut signals, mut lock) = globals.load();
    let id = ActionId(*lock);
    *lock += 1;
    let action = Arc::from(action);
    without_signal(signal, || {
        match signals.entry(signal) {
            Entry::Occupied(mut occupied) => {
                assert!(occupied.get_mut().actions.insert(id, action).is_none());
            }
            Entry::Vacant(place) => {
                let mut slot = Slot::new(signal)?;
                slot.actions.insert(id, action);
                place.insert(slot);
            }
        }

        globals.store(signals, lock);

        Ok(())
    })?;

    Ok(SigId { signal, action: id })
}

pub fn unregister(id: SigId) -> bool {
    let globals = GlobalData::ensure();
    let (mut signals, lock) = globals.load();
    let mut replace = false;
    if let Some(slot) = signals.get_mut(&id.signal) {
        replace = slot.actions.remove(&id.action).is_some();
    }
    if replace {
        globals.store(signals, lock);
    }
    replace
}
