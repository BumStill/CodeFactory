// SPDX-License-Identifier: Apache-2.0
//! Synthetic HTTP fixture shared by the repeated-open test and its transport regression.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, atomic::{AtomicBool, Ordering}};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(super) struct RepeatFixture {
    pub address: SocketAddr,
    accepted: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl RepeatFixture {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, accepted) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let server = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let mut workers = Vec::new();
            while !server_stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                };
                if server_stop.load(Ordering::SeqCst) { break; }
                let _ = tx.send(());
                workers.push(thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
                    let mut request = [0; 4096];
                    let count = match stream.read(&mut request) {
                        Ok(count) if count > 0 => count,
                        _ => return, // An empty preconnect is not an HTTP request.
                    };
                    if !request[..count].starts_with(b"GET ") { return; }
                    let body = "<html><head><title>Repeat Fixture</title></head><body><h1>Same document twice</h1></body></html>";
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }));
            }
            for worker in workers { worker.join().unwrap(); }
        });
        Self { address, accepted, stop, server: Some(server) }
    }

    pub fn idle_preconnect(&self) -> TcpStream {
        while self.accepted.try_recv().is_ok() {}
        let stream = TcpStream::connect(self.address).unwrap();
        self.accepted.recv_timeout(Duration::from_secs(5)).unwrap();
        stream
    }
}

impl Drop for RepeatFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(server) = self.server.take() { server.join().unwrap(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_preconnect_does_not_block_real_requests() {
        let fixture = RepeatFixture::start();
        let idle = fixture.idle_preconnect();
        let mut request = TcpStream::connect(fixture.address).unwrap();
        request.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        request.write_all(b"GET /non-example HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        let result = request.read_to_string(&mut response);
        // Release the injected empty connection before teardown, including on failure.
        drop(idle);
        assert!(result.is_ok(), "an idle preconnect blocked a real request: {result:?}");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.ends_with("</body></html>"), "{response}");
        assert!(response.contains("Same document twice"), "{response}");
    }
}
