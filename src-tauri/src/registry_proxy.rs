// SPDX-License-Identifier: Apache-2.0
//! 宿主出口代理。只绑定 127.0.0.1，只放行白名单主机。
//!
//! 自动放行的沙箱命令把 HTTP(S)_PROXY 指到这里。代理不解密 TLS：
//! HTTPS 只处理 CONNECT，明文 HTTP 只放行 GET 与 HEAD。

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const DEFAULT_HOSTS: &[&str] = &[
    "registry.npmjs.org",
    "registry.npmmirror.com",
    "repo.npmmirror.com",
    "index.crates.io",
    "static.crates.io",
    "crates.io",
    "pypi.org",
    "files.pythonhosted.org",
    "pypi.python.org",
    "proxy.golang.org",
    "sum.golang.org",
    "repo1.maven.org",
    "repo.maven.apache.org",
    "mirrors.aliyun.com",
    "mirrors.cloud.tencent.com",
    "mirrors.tuna.tsinghua.edu.cn",
    "mirrors.ustc.edu.cn",
    "mirrors.huaweicloud.com",
];

struct ProxyState {
    port: u16,
    extra_hosts: Vec<String>,
}

static PROXY: OnceLock<Mutex<ProxyState>> = OnceLock::new();

fn state() -> &'static Mutex<ProxyState> {
    PROXY.get_or_init(|| {
        let port = bind_and_serve().unwrap_or(0);
        Mutex::new(ProxyState {
            port,
            extra_hosts: Vec::new(),
        })
    })
}

/// 当前监听端口。绑定失败时返回 `None`，调用方必须退回审批，不能改走全网。
pub fn port() -> Option<u16> {
    let guard = state().lock().unwrap_or_else(|error| error.into_inner());
    (guard.port != 0).then_some(guard.port)
}

pub fn ensure_started() -> Result<u16, String> {
    if let Ok(value) = std::env::var("CODEFACTORY_REGISTRY_PROXY_PORT") {
        let port = value
            .parse::<u16>()
            .map_err(|_| "CODEFACTORY_REGISTRY_PROXY_PORT 不是有效端口，已拒绝放宽网络".to_string())?;
        if port == 0 {
            return Err("CODEFACTORY_REGISTRY_PROXY_PORT 为 0，已拒绝放宽网络".into());
        }
        return Ok(port);
    }
    port().ok_or_else(|| "宿主出口代理未能绑定 127.0.0.1，命令退回审批".into())
}

/// 设置页增删的主机。空字符串忽略。不写 forge 默认主机。
pub fn set_extra_hosts(hosts: &[String]) {
    let mut guard = state().lock().unwrap_or_else(|error| error.into_inner());
    guard.extra_hosts = hosts
        .iter()
        .filter_map(|host| {
            let trimmed = host.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Some(parsed) = host_from_url_token(trimmed) {
                return Some(parsed);
            }
            let bare = trimmed.trim_end_matches('.').to_ascii_lowercase();
            if bare.contains('/') || bare.contains(':') || bare.contains(' ') {
                None
            } else {
                Some(bare)
            }
        })
        .collect();
}

pub fn default_hosts() -> &'static [&'static str] {
    DEFAULT_HOSTS
}

/// 启动时由宿主（沙箱外）读取用户的 registry / 源配置，把主机并进白名单。
pub fn absorb_user_registries() {
    let mut found = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        collect_hosts_from_text(&read_lossy(&home.join(".npmrc")), &mut found);
        collect_hosts_from_text(&read_lossy(&home.join(".cargo/config.toml")), &mut found);
        collect_hosts_from_text(&read_lossy(&home.join(".cargo/config")), &mut found);
        collect_hosts_from_text(&read_lossy(&home.join(".pip/pip.conf")), &mut found);
        collect_hosts_from_text(&read_lossy(&home.join("pip/pip.conf")), &mut found);
        if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from) {
            collect_hosts_from_text(&read_lossy(&config.join("pip/pip.conf")), &mut found);
        }
    }
    if found.is_empty() {
        return;
    }
    let mut guard = state().lock().unwrap_or_else(|error| error.into_inner());
    for host in found {
        if !guard.extra_hosts.iter().any(|existing| existing == &host) {
            guard.extra_hosts.push(host);
        }
    }
}

fn read_lossy(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn collect_hosts_from_text(text: &str, out: &mut Vec<String>) {
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ',')) {
        let token = token.trim();
        if let Some(host) = host_from_url_token(token) {
            if !out.iter().any(|existing| existing == &host) {
                out.push(host);
            }
        }
    }
}

fn host_from_url_token(token: &str) -> Option<String> {
    let rest = token
        .strip_prefix("https://")
        .or_else(|| token.strip_prefix("http://"))?;
    let host = rest.split(['/', ':', '?']).next()?.trim().trim_end_matches('.');
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

pub fn host_allowed(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if DEFAULT_HOSTS.iter().any(|allowed| host_matches(&host, allowed)) {
        return true;
    }
    let guard = state().lock().unwrap_or_else(|error| error.into_inner());
    guard
        .extra_hosts
        .iter()
        .any(|allowed| host_matches(&host, allowed))
}

fn host_matches(host: &str, allowed: &str) -> bool {
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

fn bind_and_serve() -> Result<u16, ()> {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).map_err(|_| ())?;
    let port = listener.local_addr().map_err(|_| ())?.port();
    listener.set_nonblocking(false).ok();
    std::thread::Builder::new()
        .name("registry-proxy".into())
        .spawn(move || serve(listener))
        .map_err(|_| ())?;
    Ok(port)
}

fn serve(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || {
            let _ = handle_client(stream);
        });
    }
}

fn handle_client(mut client: TcpStream) -> std::io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut buffer = [0_u8; 8192];
    let read = client.read(&mut buffer)?;
    if read == 0 {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&buffer[..read]);
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let (host, port, tunneled) = match parse_target(method, target) {
        Some(value) => value,
        None => return deny(&mut client, "无法识别目标"),
    };
    if !port_allowed(port, method) {
        return deny(&mut client, "该端口不在自动放行范围内");
    }
    if !host_allowed(&host) {
        return deny(&mut client, "目标主机不在注册表白名单内");
    }
    let mut upstream = match connect_host(&host, port) {
        Ok(stream) => stream,
        Err(_) => return deny(&mut client, "无法连接目标主机"),
    };
    if tunneled {
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
        client.flush()?;
    } else {
        upstream.write_all(&buffer[..read])?;
    }
    let mut outgoing = upstream.try_clone()?;
    let mut returning = client.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut upstream, &mut returning);
    });
    let _ = std::io::copy(&mut client, &mut outgoing);
    Ok(())
}

fn parse_target(method: &str, target: &str) -> Option<(String, u16, bool)> {
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = target.rsplit_once(':')?;
        return Some((host.to_string(), port.parse().ok()?, true));
    }
    if !matches!(method, "GET" | "HEAD" | "get" | "head") {
        return None;
    }
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))?;
    let (hostport, _) = rest.split_once('/').unwrap_or((rest, ""));
    if let Some((host, port)) = hostport.rsplit_once(':') {
        Some((host.to_string(), port.parse().ok()?, false))
    } else {
        Some((hostport.to_string(), 80, false))
    }
}

fn port_allowed(port: u16, method: &str) -> bool {
    if method.eq_ignore_ascii_case("CONNECT") {
        port == 443
    } else {
        port == 80
    }
}

fn connect_host(host: &str, port: u16) -> std::io::Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no address");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn deny(client: &mut TcpStream, reason: &str) -> std::io::Result<()> {
    let body = format!("出口代理拒绝：{reason}");
    let response = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
    );
    client.write_all(response.as_bytes())?;
    client.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h4_default_allowlist_includes_public_registries_and_not_forges() {
        assert!(host_allowed("registry.npmjs.org"));
        assert!(host_allowed("index.crates.io"));
        assert!(host_allowed("files.pythonhosted.org"));
        assert!(host_allowed("proxy.golang.org"));
        assert!(host_allowed("repo1.maven.org"));
        assert!(host_allowed("mirrors.aliyun.com"));
        assert!(!host_allowed("github.com"));
        assert!(!host_allowed("api.github.com"));
        assert!(!host_allowed("gitlab.com"));
        assert!(!host_allowed("ssh.github.com"));
    }

    #[test]
    fn h4_user_registry_host_is_added_and_blank_is_ignored() {
        set_extra_hosts(&["https://example-registry.invalid".into(), "  ".into()]);
        assert!(host_allowed("example-registry.invalid"));
        set_extra_hosts(&[]);
    }

    #[test]
    fn h4_proxy_rejects_unlisted_host_and_accepts_decision() {
        let port = ensure_started().expect("proxy binds");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"CONNECT github.com:443 HTTP/1.1\r\nHost: github.com:443\r\n\r\n")
            .unwrap();
        let mut buf = [0_u8; 512];
        let n = stream.read(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf[..n]);
        assert!(text.contains("403"), "{text}");
        assert!(text.contains("出口代理拒绝"), "{text}");
    }

    #[test]
    fn h4_plain_post_is_not_forwarded() {
        assert!(parse_target("POST", "http://pypi.org/simple").is_none());
        assert!(parse_target("GET", "http://pypi.org/simple").is_some());
        assert!(!port_allowed(443, "GET"));
        assert!(port_allowed(443, "CONNECT"));
        assert!(port_allowed(80, "HEAD"));
        assert!(!port_allowed(22, "CONNECT"));
    }
}
