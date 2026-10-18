// SPDX-License-Identifier: Apache-2.0
//! E2E-004 synthetic provider socket setup.
//!
//! The fixture listener is non-blocking so its accept loop can poll against a
//! deadline. Windows inherits the listener's non-blocking mode into the
//! accepted socket, so the server's first `read` on a request that has not
//! fully arrived returns `WSAEWOULDBLOCK` (10035) and the provider thread
//! panics. A read timeout alone does not clear non-blocking mode, therefore
//! accepted sockets must be put back into blocking mode explicitly.

use std::net::TcpStream;
use std::time::Duration;

/// Prepare an accepted provider socket for the fixture's blocking request read.
///
/// Blocking mode is restored *before* the timeout is set: on Windows an
/// accepted socket inherits the listener's non-blocking mode and a read
/// timeout does not clear it, so a partially arrived request would otherwise
/// fail with `WSAEWOULDBLOCK` instead of waiting for the remaining bytes.
pub(super) fn configure_provider_socket(socket: &TcpStream) -> std::io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Regression for the Windows failure: an accepted socket carries the
    /// listener's non-blocking mode, and a fragmented request must still be
    /// read to completion instead of failing with `WouldBlock`.
    #[test]
    fn accepted_socket_reads_fragmented_request_after_nonblocking_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(accepted) => break accepted,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        };
        // Model Windows accept inheritance on every platform, deterministically.
        socket.set_nonblocking(true).unwrap();
        configure_provider_socket(&socket).unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            client.write_all(b"first ").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            client.write_all(b"second").unwrap();
        });
        let mut actual = [0; 12];
        let result = socket.read_exact(&mut actual);
        writer.join().unwrap();
        result.unwrap();
        assert_eq!(&actual, b"first second");
    }
}
