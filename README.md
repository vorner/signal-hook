# Signal-hook

[![Actions Status](https://github.com/vorner/signal-hook/workflows/test/badge.svg)](https://github.com/vorner/signal-hook/actions)
[![codecov](https://codecov.io/gh/vorner/signal-hook/branch/master/graph/badge.svg?token=3KA3R2D9fV)](https://codecov.io/gh/vorner/signal-hook)
[![docs](https://docs.rs/signal-hook/badge.svg)](https://docs.rs/signal-hook)

Library for safe and correct Unix signal handling in Rust.

Unix signals are inherently hard to handle correctly, for several reasons:

* They are a global resource. If a library wants to set its own signal handlers,
  it risks disturbing some other library. It is possible to chain the previous
  signal handler, but then it is impossible to remove the old signal handlers
  from the chains in any practical manner.
* They can be called from whatever thread, requiring synchronization. Also, as
  they can interrupt a thread at any time, making most handling race-prone.
* According to the POSIX standard, the set of functions one may call inside a
  signal handler is limited to very few of them. To highlight, mutexes (or other
  locking mechanisms) and memory allocation and deallocation are *not* allowed.

This library aims to solve some of the problems. It provides a global registry
of actions performed on arrival of signals. It is possible to register multiple
actions for the same signal and it is possible to remove the actions later on.
If there was a previous signal handler when the first action for a signal is
registered, it is chained (but the original one can't be removed).

Besides the basic registration of an arbitrary action, several helper actions
are provided to cover the needs of the most common use cases, available from
safe Rust.

For further details, see the [documentation](https://docs.rs/signal-hook).

## Example

(This likely does a lot more than you need in each individual application, it's
more of a show-case of what everything is possible, not of what you need to do
each time).

```rust
use std::io::Error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use signal_hook::consts::signal::*;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook::flag;
// A friend of the Signals iterator, but can be customized by what we want yielded about each
// signal.
use signal_hook::iterator::SignalsInfo;
use signal_hook::iterator::exfiltrator::WithOrigin;
use signal_hook::low_level;

fn main() -> Result<(), Error> {
    // Make sure double CTRL+C and similar kills
    let term_now = Arc::new(AtomicBool::new(false));
    for sig in TERM_SIGNALS {
        // When terminated by a second term signal, exit with exit code 1.
        // This will do nothing the first time (because term_now is false).
        flag::register_conditional_shutdown(*sig, 1, Arc::clone(&term_now))?;
        // But this will "arm" the above for the second time, by setting it to true.
        // The order of registering these is important, if you put this one first, it will
        // first arm and then terminate ‒ all in the first round.
        flag::register(*sig, Arc::clone(&term_now))?;
    }

    // Subscribe to all these signals with information about where they come from. We use the
    // extra info only for logging in this example (it is not available on all the OSes or at
    // all the occasions anyway, it may return `Unknown`).
    let mut sigs = vec![
        // Some terminal handling
        SIGTSTP, SIGCONT, SIGWINCH,
        // Reload of configuration for daemons ‒ um, is this example for a TUI app or a daemon
        // O:-)? You choose...
        SIGHUP,
        // Application-specific action, to print some statistics.
        SIGUSR1,
    ];
    sigs.extend(TERM_SIGNALS);
    let mut signals = SignalsInfo::<WithOrigin>::new(&sigs)?;

    // This is the actual application that'll start in its own thread. We'll control it from
    // this thread based on the signals, but it keeps running.
    // This is called after all the signals got registered, to avoid the short race condition
    // in the first registration of each signal in multi-threaded programs.
    let app = App::run_background();

    // Consume all the incoming signals. This happens in "normal" Rust thread, not in the
    // signal handlers. This means that we are allowed to do whatever we like in here, without
    // restrictions, but it also means the kernel believes the signal already got delivered, we
    // handle them in delayed manner. This is in contrast with eg the above
    // `register_conditional_shutdown` where the shutdown happens *inside* the handler.
    let mut has_terminal = true;
    for info in &mut signals {
        // Will print info about signal + where it comes from.
        eprintln!("Received a signal {:?}", info);
        match info.signal {
            SIGTSTP => {
                // Restore the terminal to non-TUI mode
                if has_terminal {
                    app.restore_term();
                    has_terminal = false;
                    // And actually stop ourselves, by a little trick.
                    low_level::raise(SIGSTOP)?;
                }
            }
            SIGCONT => {
                if !has_terminal {
                    app.claim_term();
                    has_terminal = true;
                }
            }
            SIGWINCH => app.resize_term(),
            SIGHUP => app.reload_config(),
            SIGUSR1 => app.print_stats(),
            term_sig => { // These are all the ones left
                eprintln!("Terminating");
                assert!(TERM_SIGNALS.contains(&term_sig));
                break;
            }
        }
    }

    // If during this another termination signal comes, the trick at the top would kick in and
    // terminate early. But if it doesn't, the application shuts down gracefully.
    app.wait_for_stop();

    Ok(())
}
```

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms
or conditions.
