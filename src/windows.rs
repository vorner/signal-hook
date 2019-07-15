//! Signal emulation for Windows.

use libc::c_int;
use std::collections::{BTreeMap, HashMap};
use std::io::Error;
use std::sync::Mutex;
use winapi::shared::minwindef::{BOOL, DWORD, FALSE, TRUE};
use winapi::um::consoleapi::SetConsoleCtrlHandler;
use winapi::um::wincon::{CTRL_BREAK_EVENT, CTRL_C_EVENT};

// From Windows CRT...
// SIGINT = 2
// SIGILL = 4
// SIGFPE = 8
// SIGSEGV = 11
// SIGTERM = 15
// SIGBREAK = 21
// SIGABRT = 22
pub use libc::{SIGABRT, SIGFPE, SIGILL, SIGINT, SIGSEGV, SIGTERM};

const DUMMY_OFFSET: c_int = 64;

// Define as Linux x86 signal number + 10000
/// Dummy value for SIGALRM which is missing in Windows.
pub const SIGALRM: c_int = DUMMY_OFFSET + 14;
/// Dummy value for SIGBUS which is missing in Windows.
pub const SIGBUS: c_int = DUMMY_OFFSET + 7;
/// Dummy value for SIGCHLD which is missing in Windows.
pub const SIGCHLD: c_int = DUMMY_OFFSET + 17;
/// Dummy value for SIGCONT which is missing in Windows.
pub const SIGCONT: c_int = DUMMY_OFFSET + 18;
/// Dummy value for SIGHUP which is missing in Windows.
pub const SIGHUP: c_int = DUMMY_OFFSET + 1;
/// Dummy value for SIGIO which is missing in Windows.
pub const SIGIO: c_int = DUMMY_OFFSET + 29;
/// Dummy value for SIGKILL which is missing in Windows.
pub const SIGKILL: c_int = DUMMY_OFFSET + 9;
/// Dummy value for SIGPIPE which is missing in Windows.
pub const SIGPIPE: c_int = DUMMY_OFFSET + 13;
/// Dummy value for SIGPROF which is missing in Windows.
pub const SIGPROF: c_int = DUMMY_OFFSET + 27;
/// Dummy value for SIGQUIT which is missing in Windows.
pub const SIGQUIT: c_int = DUMMY_OFFSET + 3;
/// Dummy value for SIGSTOP which is missing in Windows.
pub const SIGSTOP: c_int = DUMMY_OFFSET + 19;
/// Dummy value for SIGSYS which is missing in Windows.
pub const SIGSYS: c_int = DUMMY_OFFSET + 31;
/// Dummy value for SIGTRAP which is missing in Windows.
pub const SIGTRAP: c_int = DUMMY_OFFSET + 5;
/// Dummy value for SIGUSR1 which is missing in Windows.
pub const SIGUSR1: c_int = DUMMY_OFFSET + 10;
/// Dummy value for SIGUSR2 which is missing in Windows.
pub const SIGUSR2: c_int = DUMMY_OFFSET + 12;
/// Dummy value for SIGWINCH which is missing in Windows.
pub const SIGWINCH: c_int = DUMMY_OFFSET + 28;

/// List of forbidden signals.
///
/// Some signals are impossible to replace according to POSIX and some are so special that this
/// library refuses to handle them (eg. SIGSEGV). The routines panic in case registering one of
/// these signals is attempted.
///
/// See [`register`](fn.register.html).
pub const FORBIDDEN: &[c_int] = &[SIGKILL, SIGSTOP, SIGILL, SIGFPE, SIGSEGV];

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

struct GlobalData {
    // RCU isn't necessary here. Unlike posix signals, Windows Console handler is called in a separate thread.
    state: Mutex<GlobalDataState>,
}

struct GlobalDataState {
    action_id: u64,
    all_signals: HashMap<c_int, BTreeMap<ActionId, Box<Action>>>,
}

impl GlobalData {
    fn ensure() -> &'static Self {
        use std::cell::UnsafeCell;
        use std::sync::Once;

        struct GlobalCell(UnsafeCell<Option<GlobalData>>);
        unsafe impl Sync for GlobalCell {}
        static GLOBAL: GlobalCell = GlobalCell(UnsafeCell::new(None));
        static GLOBAL_ONCE: Once = Once::new();

        GLOBAL_ONCE.call_once(|| unsafe {
            *GLOBAL.0.get() = Some(GlobalData::new());
        });

        if let &Some(ref data) = unsafe { &*GLOBAL.0.get() } {
            data
        } else {
            unreachable!();
        }
    }

    fn new() -> Self {
        assert!(unsafe { SetConsoleCtrlHandler(Some(handler), TRUE) != 0 });
        GlobalData {
            state: Mutex::new(GlobalDataState::new()),
        }
    }
}
impl GlobalDataState {
    fn new() -> Self {
        Self {
            action_id: 0,
            all_signals: HashMap::new(),
        }
    }
}

extern "system" fn handler(ctrl_type: DWORD) -> BOOL {
    if let Some(signal) = to_signal(ctrl_type) {
        let data = GlobalData::ensure();
        let state = data.state.lock().unwrap();
        if let Some(all_signals) = state.all_signals.get(&signal) {
            for action in all_signals.values() {
                action();
            }

            // returning TRUE here has two effects:
            // 1. Default behavior is prevented.
            // 2. Handlers registered earlier than this are not invoked.
            //
            // Ideally we want to prevent default behavior while keeping
            // old callbacks, but it's impossible.
            //
            // And we may want to return FALSE if `all_signals` is empty.
            // I didn't do so to align with the unix implementation.
            return TRUE;
        }
    }
    FALSE
}

fn to_signal(ctrl_type: DWORD) -> Option<c_int> {
    if ctrl_type == CTRL_C_EVENT {
        Some(SIGINT)
    } else if ctrl_type == CTRL_BREAK_EVENT {
        Some(SIGTERM)
    } else {
        None
    }
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
/// If the signal is one of (see [`FORBIDDEN`]):
///
/// * `SIGKILL`
/// * `SIGSTOP`
/// * `SIGILL`
/// * `SIGFPE`
/// * `SIGSEGV`
///
/// The first two are not possible to override (and the underlying C functions simply ignore all
/// requests to do so, which smells of possible bugs, or return errors). The rest can be set, but
/// generally needs very special handling to do so correctly (direct manipulation of the
/// application's address space, `longjmp` and similar). Unless you know very well what you're
/// doing, you'll shoot yourself into the foot and this library won't help you with that.
///
/// # Errors
///
/// Since the library manipulates signals using the low-level C functions, all these can return
/// errors. Generally, the errors mean something like the specified signal does not exist on the
/// given platform ‒ ofter a program is debugged and tested on a given OS, it should never return
/// an error.
///
/// However, if an error *is* returned, there are no guarantees if the given action was registered
/// or not.
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
/// As panicking from within a signal handler would be a panic across FFI boundary (which is
/// undefined behavior), the passed handler must not panic.
///
/// If you find these limitations hard to satisfy, choose from the helper functions in submodules
/// of this library ‒ these provide safe interface to use some common signal handling patters.
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
///
/// # Examples
///
/// ```rust
/// extern crate signal_hook;
///
/// use std::io::Error;
/// use std::process;
///
/// fn main() -> Result<(), Error> {
///     let signal = unsafe { signal_hook::register(signal_hook::SIGTERM, || process::abort()) }?;
///     // Stuff here...
///     signal_hook::unregister(signal); // Not really necessary.
///     Ok(())
/// }
/// ```
pub unsafe fn register<F>(signal: c_int, action: F) -> Result<SigId, Error>
where
    F: Fn() + Sync + Send + 'static,
{
    assert!(
        !FORBIDDEN.contains(&signal),
        "Attempted to register forbidden signal {}",
        signal,
    );
    let data = GlobalData::ensure();
    let mut state = data.state.lock().unwrap();
    let action_id = ActionId(state.action_id);
    state.action_id += 1;

    let sig_id = SigId {
        signal,
        action: action_id,
    };

    let all_signals = state.all_signals.entry(signal).or_insert(BTreeMap::new());
    all_signals.insert(action_id, Box::new(action));

    Ok(sig_id)
}

/// Removes a previously installed action.
///
/// This function does nothing if the action was already removed. It returns true if it was removed
/// and false if the action wasn't found.
///
/// It can unregister all the actions installed by [`register`](fn.register.html) as well as the
/// ones from helper submodules.
///
/// # Warning
///
/// This does *not* currently return the default/previous signal handler if the last action for a
/// signal was just unregistered. That means that if you replaced for example `SIGTERM` and then
/// removed the action, the program will effectively ignore `SIGTERM` signals from now on, not
/// terminate on them as is the default action. This is OK if you remove it as part of a shutdown,
/// but it is not recommended to remove termination actions during the normal runtime of
/// application (unless the desired effect is to create something that can be terminated only by
/// SIGKILL).
pub fn unregister(id: SigId) -> bool {
    let data = GlobalData::ensure();
    let mut state = data.state.lock().unwrap();
    let mut replace = false;
    if let Some(actions) = state.all_signals.get_mut(&id.signal) {
        replace = actions.remove(&id.action).is_some();
    }
    replace
}

// Emulates kill(2) for testing `signal-hook` in Windows.
#[doc(hidden)]
pub unsafe fn __emulate_kill(pid: c_int, signal: c_int) -> c_int {
    let self_pid = ::libc::getpid();
    if pid != self_pid && pid != -self_pid && pid != 0 && pid != -1 {
        return 0;
    }

    let data = GlobalData::ensure();
    let state = data.state.lock().unwrap();
    if let Some(all_signals) = state.all_signals.get(&signal) {
        for action in all_signals.values() {
            action();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn panic_forbidden() {
        let _ = unsafe { register(SIGKILL, || ()) };
    }

    /// Check that registration works as expected and that unregister tells if it did or not.
    #[test]
    fn register_unregister() {
        let signal = unsafe { register(SIGUSR1, || ()).unwrap() };
        // It was there now, so we can unregister
        assert!(unregister(signal));
        // The next time unregistering does nothing and tells us so.
        assert!(!unregister(signal));
    }
}
