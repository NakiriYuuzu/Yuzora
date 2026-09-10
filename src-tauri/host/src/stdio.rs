//! Pollable Unix stdio. Tokio's blocking stdin worker cannot be cancelled and
//! otherwise keeps completed one-shot helpers alive until the peer sends EOF.
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::{unix::AsyncFd, AsyncRead, AsyncWrite, Interest, ReadBuf};

pub struct StdioPipe(AsyncFd<OwnedFd>);
impl StdioPipe {
    pub fn input() -> io::Result<Self> {
        Self::from_stdio(libc::STDIN_FILENO, Interest::READABLE)
    }
    pub fn output() -> io::Result<Self> {
        Self::from_stdio(libc::STDOUT_FILENO, Interest::WRITABLE)
    }
    fn from_stdio(source: i32, interest: Interest) -> io::Result<Self> {
        let fd = unsafe { libc::fcntl(source, libc::F_DUPFD_CLOEXEC, 3) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(AsyncFd::with_interest(owned, interest)?))
    }
}
impl AsyncRead for StdioPipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut ready = ready!(self.0.poll_read_ready(cx))?;
            let result = ready.try_io(|fd| {
                let available = buffer.initialize_unfilled();
                let n = unsafe {
                    libc::read(
                        fd.get_ref().as_raw_fd(),
                        available.as_mut_ptr().cast(),
                        available.len(),
                    )
                };
                if n == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match result {
                Ok(Ok(n)) => {
                    buffer.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => {}
            }
        }
    }
}
impl AsyncWrite for StdioPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut ready = ready!(self.0.poll_write_ready(cx))?;
            match ready.try_io(|fd| {
                let n = unsafe {
                    libc::write(fd.get_ref().as_raw_fd(), bytes.as_ptr().cast(), bytes.len())
                };
                if n == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => return Poll::Ready(result),
                Err(_) => {}
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
