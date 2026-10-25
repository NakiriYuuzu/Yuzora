//! Open-contract checks run only inside the parent's isolated lifecycle child.
use super::OwnedPty;
use portable_pty::{native_pty_system, PtySize};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::time::{Duration, Instant};

fn termios(fd: i32) -> libc::termios {
    let mut value = std::mem::MaybeUninit::zeroed();
    assert_eq!(unsafe { libc::tcgetattr(fd, value.as_mut_ptr()) }, 0);
    unsafe { value.assume_init() }
}

pub(super) fn verify() {
    // This private process has one test and no concurrent spawns. Close the
    // original libc reference pair before creating any child process.
    let mut master = -1;
    let mut slave = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let reference_master = unsafe { File::from_raw_fd(master) };
    let reference_slave = unsafe { File::from_raw_fd(slave) };
    let expected = termios(master);
    drop(reference_slave);
    drop(reference_master);
    let size = PtySize {
        rows: 37,
        cols: 103,
        pixel_width: 927,
        pixel_height: 481,
    };
    let pair = native_pty_system().openpty(size).unwrap();
    let actual_size = pair.master.get_size().unwrap();
    assert_eq!(
        (
            actual_size.rows,
            actual_size.cols,
            actual_size.pixel_width,
            actual_size.pixel_height
        ),
        (37, 103, 927, 481)
    );
    let actual = termios(pair.master.as_raw_fd().unwrap());
    assert_eq!(
        (
            actual.c_iflag,
            actual.c_oflag,
            actual.c_cflag,
            actual.c_lflag
        ),
        (
            expected.c_iflag,
            expected.c_oflag,
            expected.c_cflag,
            expected.c_lflag
        )
    );
    assert_eq!(actual.c_cc, expected.c_cc);
    assert_eq!(unsafe { libc::cfgetispeed(&actual) }, unsafe {
        libc::cfgetispeed(&expected)
    });
    assert_eq!(unsafe { libc::cfgetospeed(&actual) }, unsafe {
        libc::cfgetospeed(&expected)
    });
    drop(pair);

    let mut fixture = OwnedPty::spawn().unwrap();
    let master = fixture.master.as_ref().unwrap();
    assert_eq!(master.process_group_leader(), Some(fixture.pid));
    let flags = unsafe { libc::fcntl(master.as_raw_fd().unwrap(), libc::F_GETFD) };
    assert!(
        flags >= 0,
        "F_GETFD failed: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    fixture
        .input
        .as_mut()
        .unwrap()
        .write_all(b"hello\n")
        .unwrap();
    fixture.input.as_mut().unwrap().flush().unwrap();
    let mut poll = libc::pollfd {
        fd: master.as_raw_fd().unwrap(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert!(unsafe { libc::poll(&mut poll, 1, 2000) } > 0);
    let mut echo = [0; 7];
    fixture
        .output
        .as_mut()
        .unwrap()
        .read_exact(&mut echo)
        .unwrap();
    assert_eq!(&echo, b"hello\r\n");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = fixture.child.as_mut().unwrap().try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PTY input did not reach the child"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(fixture.observe().unwrap(), "reaped");
    fixture.cleanup().unwrap();
    drop(fixture);
    allocation_failure_releases_master();
    eprintln!("NATIVE_CLIENT_SETUP_GUARDS defaults geometry bidirectional-io controlling-tty cloexec allocation-failure passed");
}

fn allocation_failure_releases_master() {
    struct Limit(libc::rlimit);
    impl Drop for Limit {
        fn drop(&mut self) {
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &self.0) }, 0);
        }
    }
    let mut previous = std::mem::MaybeUninit::<libc::rlimit>::zeroed();
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, previous.as_mut_ptr()) },
        0
    );
    let previous = unsafe { previous.assume_init() };
    let restore = Limit(previous);
    let limited = libc::rlimit {
        rlim_cur: previous.rlim_cur.min(64),
        rlim_max: previous.rlim_max,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limited) }, 0);
    let mut reservations = Vec::new();
    loop {
        match File::open("/dev/null") {
            Ok(file) => reservations.push(file),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::EMFILE));
                break;
            }
        }
    }
    assert!(!reservations.is_empty());
    drop(reservations.pop()); // One free slot: master opens, slave must fail.
    for _ in 0..100 {
        assert!(native_pty_system().openpty(PtySize::default()).is_err());
        // Each failed open must return the slot used by its master.
        drop(File::open("/dev/null").expect("failed PTY open retained its master"));
    }
    drop(reservations);
    drop(restore);
}
