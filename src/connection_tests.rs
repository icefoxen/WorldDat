use std::sync::mpsc;

use bytes;
use futures;
use quinn;
use std::io::{self, Read};
use tokio_io;

use peer;

/// TODO: Why the heck do the mpsc types not implement Read or Write?
/// If they did we could easily implement AsyncRead and AsyncWrite on them
/// using `tokio_io::AllowStdIo`, I think.
///
/// Eh, part of it is that Read and Write are u8-specific, but still...
#[allow(dead_code)]
pub struct TestStream {
    send_stream: mpsc::Sender<u8>,
    recv_stream: mpsc::Receiver<u8>,
}

impl TestStream {
    #[allow(dead_code)]
    pub fn new_pair() -> (Self, Self) {
        let (s1, r1) = mpsc::channel();
        let (s2, r2) = mpsc::channel();
        let stream1 = TestStream {
            send_stream: s1,
            recv_stream: r2,
        };
        let stream2 = TestStream {
            send_stream: s2,
            recv_stream: r1,
        };
        (stream1, stream2)
    }
}

impl quinn::Write for TestStream {
    fn poll_write(&mut self, buf: &[u8]) -> Result<futures::Async<usize>, quinn::WriteError> {
        buf.iter().for_each(|b| {
            self.send_stream.send(*b).expect("Could not send byte");
        });
        Ok(futures::Async::Ready(buf.len()))
    }

    /// TODO: This should actually shut down the connection.
    fn poll_finish(&mut self) -> Result<futures::Async<()>, quinn::ConnectionError> {
        Ok(futures::Async::Ready(()))
    }

    fn reset(&mut self, _error_code: u16) {}
}

impl io::Write for TestStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        // TODO: Some sort of awesome map-collect to count the actual
        // number of successful bytes or the error might be nice.
        buf.iter().for_each(|b| {
            self.send_stream.send(*b).expect("Could not send byte");
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
}

impl tokio_io::AsyncWrite for TestStream {
    /// TODO: This should actually shut down the connection.
    fn shutdown(&mut self) -> Result<futures::Async<()>, io::Error> {
        Ok(futures::Async::Ready(()))
    }
}

impl quinn::Read for TestStream {
    fn poll_read_unordered(
        &mut self,
    ) -> Result<futures::Async<(bytes::Bytes, u64)>, quinn::ReadError> {
        let mut buf = vec![];
        // TODO: Given that the stream may be reused...  Might returning 0
        // for the bytes offset here not always be correct?
        // We probably need to count the bytes read and use that.
        self.poll_read(buf.as_mut())
            .map(|async_num| async_num.map(|_num| (bytes::Bytes::from(buf), 0)))
    }

    fn poll_read(&mut self, buf: &mut [u8]) -> Result<futures::Async<usize>, quinn::ReadError> {
        // TODO: Handle quinn error type correctly!
        self.read(buf)
            .map(|res| futures::Async::Ready(res))
            .map_err(|_e| quinn::ReadError::Finished)
    }

    /// TODO: This should actually shut down the connection.
    fn stop(&mut self, _error_code: u16) {}
}

impl io::Read for TestStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        if buf.len() == 0 {
            return Ok(0);
        }
        for i in 0..buf.len() {
            match self.recv_stream.try_recv() {
                Ok(b) => {
                    buf[i] = b;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // TODO: Check for off-by-one
                    return Ok(i);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "Channel closed",
                    ));
                }
            }
        }
        return Ok(buf.len());
    }
}

impl tokio_io::AsyncRead for TestStream {}

impl peer::SendStream for TestStream {}
impl peer::Stream for TestStream {}

#[cfg(test)]
mod tests {
    use futures;
    use quinn;

    use super::*;

    #[test]
    fn test_io_read_write() {
        use std::io::{Read, Write};
        let (mut s1, mut s2) = TestStream::new_pair();
        let msg = b"Foo!";
        let sent_bytes = s1.write(msg).unwrap();
        assert_eq!(sent_bytes, msg.len());
        s1.flush().unwrap();
        let recv_buf = &mut vec![0; 4];
        let received_bytes = s2.read(recv_buf).unwrap();
        assert_eq!(received_bytes, msg.len());
        assert_eq!(recv_buf, msg);

        let received_bytes = s2.read(recv_buf).unwrap();
        assert_eq!(received_bytes, 0);
    }

    #[test]
    fn test_async_io_read_write() {
        use tokio_io::{AsyncRead, AsyncWrite};
        let (mut s1, mut s2) = TestStream::new_pair();
        let msg = b"Foo!";
        let sent_bytes = s1.poll_write(msg).unwrap();
        assert_eq!(sent_bytes, futures::Async::Ready(msg.len()));
        s1.poll_flush().unwrap();
        let recv_buf = &mut vec![0; 4];
        let received_bytes = s2.poll_read(recv_buf).unwrap();
        assert_eq!(received_bytes, futures::Async::Ready(msg.len()));
        assert_eq!(recv_buf, msg);

        let received_bytes = s2.poll_read(recv_buf).unwrap();
        assert_eq!(received_bytes, futures::Async::Ready(0));
    }

    #[test]
    fn test_quinn_io_read_write() {
        use quinn::{Read, Write};
        let (mut s1, mut s2) = TestStream::new_pair();
        let msg = b"Foo!";
        let sent_bytes = s1.poll_write(msg).unwrap();
        assert_eq!(sent_bytes, futures::Async::Ready(msg.len()));
        let recv_buf = &mut vec![0; 4];
        let received_bytes = s2.poll_read(recv_buf).unwrap();
        assert_eq!(received_bytes, futures::Async::Ready(msg.len()));
        assert_eq!(recv_buf, msg);

        let received_bytes = s2.poll_read(recv_buf).unwrap();
        assert_eq!(received_bytes, futures::Async::Ready(0));
    }
}
