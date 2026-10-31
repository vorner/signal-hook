#![cfg(all(unix, feature = "extended-siginfo"))]

use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGCHLD, SIGCONT, SIGSTOP, SIGTERM};
use signal_hook::iterator::exfiltrator::WithOrigin;
use signal_hook::iterator::SignalsInfo;
use signal_hook::low_level::siginfo::{Cause, Chld, Origin};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn next_origin(signals: &mut SignalsInfo<WithOrigin>, pid: Option<libc::pid_t>) -> Origin {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for origin in signals.pending() {
            if pid.is_none() || origin.process.map(|process| process.pid) == pid {
                return origin;
            }
        }
        assert!(Instant::now() < deadline, "missing SIGCHLD for {:?}", pid);
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn child_status_and_user_signal() {
    let _lock = serial_test::lock();
    let mut signals = SignalsInfo::<WithOrigin>::new([SIGCHLD]).unwrap();

    // A user-sent SIGCHLD does not contain a child's status in its siginfo union.
    signal_hook::low_level::raise(SIGCHLD).unwrap();
    let origin = next_origin(&mut signals, None);
    assert_eq!(origin.process.and_then(|process| process.status), None);

    for status in [0, 7] {
        let mut child = ChildGuard(
            Command::new("sh")
                .arg("-c")
                .arg(format!("exit {}", status))
                .spawn()
                .unwrap(),
        );
        let origin = next_origin(&mut signals, Some(child.0.id() as libc::pid_t));
        assert_eq!(origin.cause, Cause::Chld(Chld::Exited));
        assert_eq!(origin.process.unwrap().status, Some(status));
        assert_eq!(child.0.wait().unwrap().code(), Some(status));
    }

    let mut child = ChildGuard(Command::new("sleep").arg("30").spawn().unwrap());
    let pid = child.0.id() as libc::pid_t;
    for (signal, cause) in [(SIGSTOP, Chld::Stopped), (SIGCONT, Chld::Continued)] {
        // The pid belongs to our live child and the signal is valid.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
        // macOS resumes the child without sending a continued-state SIGCHLD.
        #[cfg(target_os = "macos")]
        if signal == SIGCONT {
            continue;
        }
        let origin = next_origin(&mut signals, Some(pid));
        assert_eq!(origin.cause, Cause::Chld(cause));
        assert_eq!(origin.process.unwrap().status, Some(signal));
        if signal == SIGSTOP {
            let mut status = 0;
            // The status pointer is valid and this only acknowledges our stopped child.
            assert_eq!(
                unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
                pid
            );
            assert!(libc::WIFSTOPPED(status));
        }
    }

    // The pid still belongs to our child; SIGTERM ends the sleeping process.
    assert_eq!(unsafe { libc::kill(pid, SIGTERM) }, 0);
    let origin = next_origin(&mut signals, Some(pid));
    assert_eq!(origin.cause, Cause::Chld(Chld::Killed));
    assert_eq!(origin.process.unwrap().status, Some(SIGTERM));
    assert!(!child.0.wait().unwrap().success());
}
