#![doc(html_root_url = "https://docs.rs/signal-hook/0.1.0/signal-hook/")]
#![deny(missing_docs)]

//! Library for easy and safe unix signal handling
//!
//! Unix signals are inherently hard to handle correctly, for several reasons:
//!
//! * They are a global resource. If a library wants to set its own signal handlers, it risks
//!   disturbing some other library. It is possible to chain the previous signal handler, but then
//!   it is impossible to remove the old signal handlers from the chains in any practical manner.
//! * They can be called from whatever thread, requiring synchronization. Also, as they can
//!   interrupt a thread at any time, making most handling race-prone.
//! * According to the POSIX standard, the set of functions one may call inside a signal handler is
//!   limitted to very few of them. To highlight, mutexes (or other locking mechanisms) and memory
//!   allocation and deallocation is *not* allowed.
//!
//! This library aims to solve some of the problems. It provides a global registry of actions
//! performed on arrival of signals. It is possible to register multiple actions for the same
//! signal and it is possible to remove the actions later on. If there was a previous signal
//! handler when the first action for a signal is registered, it is chained (but the original one
//! can't be removed).
//!
//! The main function of the library is [`register`](fn.register.html).
//!
//! It also offers several common actions one might want to register, implemented in the correct
//! way. They are scattered through submodules and have the same limitations and characteristics as
//! the [`register`](fn.register.html) function.
//!
//! Unlike other Rust libraries for signal handling, this should be flexible enough to handle all
//! the common and useful patterns.
//!
//! # Warning
//!
//! Even with this library, you should thread with care. It does not eliminate all the problems
//! mentioned above.
//!
//! Also, note that the OS may collate multiple instances of the same signal into just one call of
//! the signal handler. Furthermore, some abstractions implemented here also naturally collate
//! multiple instances of the same signal. The general guarantee is, if there was at least one
//! signal of the given number delivered, an action will be taken, but it is not specified how many
//! times ‒ signals work mostly as kind of „wake up now“ nudge, if the application is slow to wake
//! up, it may be nudged multiple times before it does so.
//!
//! # Signal limitations
//!
//! OS limits still apply ‒ it is not possible to redefine certain signals (eg. `SIGKILL` or
//! `SIGSTOP`) and it is probably a *very* stupid idea to touch certain other ones (`SIGSEGV`,
//! `SIGFPE`, `SIGILL`). Therefore, this library will panic if any attempt at manipulating these is
//! made. There are some use cases for redefining the latter ones, but these are not well served by
//! this library and you really *really* have to know what you're doing and are generally on your
//! own doing that.
//!
//! # Signal masks
//!
//! As the library uses `sigaction` under the hood, signal masking works as expected (eg. with
//! `sigprocmask`). This means, signals will *not* be delivered if the signal is masked in all
//! program's threads.
//!
//! # Portability
//!
//! It should work on any POSIX.1-2001 system, which are all the major big OSes with the notable
//! exception of Windows.
//!
//! Windows has some limited support for signals, patches to include support are welcome.

// TODO: Document the internals

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

use arc_swap::ArcSwap;
use libc::{c_int, c_void, sigaction, siginfo_t, sigset_t, SIG_BLOCK, SIG_SETMASK};

pub mod flag;
pub mod iterator;
pub mod pipe;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct ActionId(u64);

/// An ID of registered action.
///
/// This is returned by all the registration routines and can be used to remove the action later on
/// with a call to [`unregister`](fn.unregister.html).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SigId {
    signal: c_int,
    action: ActionId,
}

type Action = Fn() + Send + Sync;

#[derive(Clone)]
struct Slot {
    prev: sigaction,
    actions: BTreeMap<ActionId, Arc<Action>>,
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
    fn store(&self, signals: AllSignals, lock: MutexGuard<u64>) {
        let signals = Arc::new(signals);
        // Strictly speaking, we are behind a mutex now (because of setting up the signal handlers
        // and such), so the rcu would not be strictly necessary. But we use the unwrap part of it
        // ‒ to make sure the old value gets freed outside of the signal handler.
        self.all_signals.rcu_unwrap(|_| Arc::clone(&signals));
        drop(lock);
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

/// Registers an arbitrary action for the given signal.
///
/// This makes sure there's a signal handler for the given signal. It then adds the action to the
/// ones called each time the signal is delivered. If multiple actions are set for the same signal,
/// all are called, in the order of registration.
///
/// If there was a previous signal handler for the given signal, it is chained ‒ it will be called
/// as part of this library's signal handler, before any actions set through this function.
///
/// On success, the function returns an ID that can be used to remove the action again with
/// [`unregister`](fn.unregister.html).
///
/// # Panics
///
/// If the signal is one of:
///
/// * `SIGKILL`
/// * `SIGSTOP`
/// * `SIGILL`
/// * `SIGFPE`
/// * `SIGSEGV`
///
/// The first two are not possible to override (and the underlying C functions simply ignore all
/// requests to do so, which smells of possible bugs). The rest can be set, but generally needs
/// very special handling to do so correctly (direct manipulation of the application's address
/// space, `longjmp` and similar). Unless you know very well what you're doing, you'll shoot
/// yourself into the foot and this library won't help you with that.
///
/// # Errors
///
/// Since the library manipulates signals using the low-level C functions, all these can return
/// errors. Generally, the errors mean something like the specified signal does not exist on the
/// given platform ‒ ofter a program is debugged and tested on a given OS, it should never return
/// an error.
///
/// However, if an error *is* returned, there are no guarantees if the given action was registered.
///
/// # Unsafety
///
/// This function is unsafe, because the `action` is run inside a signal handler. The set of
/// functions allowed to be called from within is very limited (they are called signal-safe
/// functions by POSIX). These specifically do *not* contain mutexes and memory
/// allocation/deallocation. They *do* contain routines to terminate the program, to further
/// manipulate signals (by the low-level functions, not by this library) and to read and write file
/// descriptors. Calling program's own functions consisting only of these is OK, as is manipulating
/// program's variables ‒ however, as the action can be called on any thread that does not have the
/// given signal masked (by default no signal is masked on any thread), and mutexes are a no-go,
/// this is harder than it looks like at first.
///
/// Furthermore, panic from within the action will terminate the program ‒ unwinding out of a
/// signal handler is UB (it's panic across FFI boundary, strictly speaking).
///
/// If you find these limitations hard to satisfy, choose from the helper functions in submodules
/// of this library.
///
/// # Race condition
///
/// Currently, there's a short race condition. If this is the first action for the given signal,
/// there was another signal handler previously and the signal comes into a different thread during
/// this function, it can happen the original handler is not chained in this one instance.
///
/// This is considered unimportant problem, since most programs install their signal handlers
/// during startup, before the signals effectively do anything. If you want to avoid the race
/// condition completely, initialize all signal handling before starting any threads.
///
/// # Performance
///
/// Even when it is possible to repeatedly install and remove actions during the lifetime of a
/// program, the installation and removal is considered a slow operation and should not be done
/// very often. Also, there's limited (though huge) amount of distinct IDs (they are `u64`).
pub unsafe fn register<F>(signal: c_int, action: F) -> Result<SigId, Error>
where
    F: Fn() + Sync + Send + 'static,
{
    // TODO: Panic if signal is one of the forbidden.
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

/// Removes a previously installed action.
///
/// This function does nothing if the action was already removed. It returns true if it was removed
/// and false if the action wasn't found.
///
/// It can unregister all the actions installed by [`register`](fn.register.html) as well as the
/// ones from helper submodules.
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
