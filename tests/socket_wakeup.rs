#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    feature = "iterator"
))]

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use signal_hook::consts::SIGUSR1;
use signal_hook::iterator::Signals;
use signal_hook::low_level::raise;

#[test]
fn iterator_wakes_when_socket_sends_are_denied() {
    const CHILD: &str = "SIGNAL_HOOK_SOCKET_WAKEUP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let mut signals = Signals::new([SIGUSR1]).unwrap();
        let handle = signals.handle();
        let (received, receive) = mpsc::channel();
        let (stopped, stop) = mpsc::channel();
        let worker = thread::spawn(move || {
            for signal in signals.forever() {
                received.send(signal).unwrap();
            }
            stopped.send(()).unwrap();
        });
        // Give the iterator time to block on its self-pipe before raising the
        // signal, so pending flags alone cannot make this test pass.
        thread::sleep(Duration::from_millis(100));
        raise(SIGUSR1).unwrap();
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(2)).unwrap(),
            SIGUSR1
        );
        // Let the iterator block again so close() must wake it independently.
        thread::sleep(Duration::from_millis(100));
        handle.close();
        stop.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        return;
    }

    // Apply the filter in a subprocess so neither signal handlers nor seccomp
    // restrictions affect other tests. A timeout also bounds a broken close().
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "iterator_wakes_when_socket_sends_are_denied",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: this post-fork callback only uses stack data and prctl.
    unsafe {
        command.pre_exec(|| {
            let mut filter = [
                libc::sock_filter {
                    code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                    jt: 0,
                    jf: 0,
                    k: 0, // seccomp_data.nr
                },
                libc::sock_filter {
                    code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 1,
                    k: libc::SYS_sendto as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ALLOW,
                },
            ];
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_mut_ptr(),
            };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == -1
                || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("iterator failed to shut down: {:?}", output);
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output);
}
