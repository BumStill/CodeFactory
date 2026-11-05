// SPDX-License-Identifier: Apache-2.0
// Compile the production shared HTTP helper against its two provider variants.
mod config {
    pub mod settings {
        pub enum GitProvider {
            Github,
            Gitlab,
        }
    }
}
#[path = "../src/git_remote/client.rs"]
mod client;
use std::io::{Read, Write};
async fn request(response: String, limit: usize) -> Result<serde_json::Value, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let n = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..n]);
        assert!(request.starts_with("GET /fixed "));
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer synthetic"));
        stream.write_all(response.as_bytes()).unwrap();
    });
    let mut client = client::RemoteGitClient::new(
        &format!("http://{addr}"),
        "synthetic",
        config::settings::GitProvider::Github,
    );
    client.inner = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let result = client.get_bounded("/fixed", limit).await;
    server.join().unwrap();
    result
}
#[tokio::test]
async fn bounded_get_rejects_redirect_oversize_and_does_not_expose_body() {
    let body = "provider-private-message";
    assert!(request(format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: {}\r\n\r\n{body}",body.len()),100).await.unwrap_err().contains("rejected"));
    assert!(request(
        "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n1234567890".into(),
        5
    )
    .await
    .unwrap_err()
    .contains("limit"));
    let error = request(
        format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
        100,
    )
    .await
    .unwrap_err();
    assert!(!error.contains(body));
    assert_eq!(
        request(
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n{\"id\":7}".into(),
            100
        )
        .await
        .unwrap()["id"],
        7
    );
}
