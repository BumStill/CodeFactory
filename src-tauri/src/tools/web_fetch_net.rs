// SPDX-License-Identifier: Apache-2.0
//! M195 — route `web_fetch` through the machine's own proxy configuration.
//!
//! `reqwest`'s default client already honours `HTTP(S)_PROXY` / `ALL_PROXY`
//! (including `NO_PROXY`). What it never reads is the *operating system's*
//! proxy settings, and corporate machines usually publish a PAC (auto-config)
//! script rather than an environment variable — so the browser reaches a page
//! that `web_fetch` cannot. This module supplies that layer:
//!
//! * [`ProxyEnv`] — a snapshot of the proxy environment, so the routing
//!   decision is a pure function a test can drive.
//! * [`SystemProxy`] — what the OS says (fixed proxy per scheme + PAC URL +
//!   exceptions), parsed from `scutil --proxy` on macOS and from
//!   `reg query …\Internet Settings` on Windows.
//! * [`evaluate_pac`] — a deliberately small `FindProxyForURL` evaluator for
//!   the subset real corporate scripts use.
//! * [`route_for`] — environment first, then the system configuration, then a
//!   documented fallback.
//!
//! Nothing here bypasses a network policy: a proxy that cannot be used is
//! reported, never worked around.

use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Environment,
    SystemFixed,
    Pac,
}

impl Source {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Source::Environment => "environment variables",
            Source::SystemFixed => "the system proxy settings",
            Source::Pac => "the PAC script",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// Leave proxy selection to `reqwest` (environment variables, `NO_PROXY`
    /// included) — the behaviour that already worked.
    Environment,
    /// Fetch without a proxy. `note` records why, for logs.
    Direct { note: Option<String> },
    /// Route this request through `url` (`http://host:port`).
    Proxy { url: String, source: Source },
    /// A proxy is configured but this build cannot use it. The caller must
    /// report this instead of silently connecting directly.
    Unsupported { reason: String },
}

// --------------------------------------------------------------------- env

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyEnv {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub no_proxy: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvDecision {
    UseProxy,
    Bypassed,
    NotConfigured,
}

impl ProxyEnv {
    pub(crate) fn from_process() -> Self {
        fn read(names: &[&str]) -> Option<String> {
            names
                .iter()
                .find_map(|name| std::env::var(name).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        }
        Self {
            http_proxy: read(&["HTTP_PROXY", "http_proxy"]),
            https_proxy: read(&["HTTPS_PROXY", "https_proxy"]),
            all_proxy: read(&["ALL_PROXY", "all_proxy"]),
            no_proxy: read(&["NO_PROXY", "no_proxy"]),
        }
    }

    fn candidate(&self, url: &Url) -> Option<&String> {
        if url.scheme() == "https" {
            self.https_proxy
                .as_ref()
                .or(self.http_proxy.as_ref())
                .or(self.all_proxy.as_ref())
        } else {
            self.http_proxy
                .as_ref()
                .or(self.all_proxy.as_ref())
                .or(self.https_proxy.as_ref())
        }
    }

    fn decision(&self, url: &Url) -> EnvDecision {
        if self.candidate(url).is_none() {
            return EnvDecision::NotConfigured;
        }
        let host = url.host_str().unwrap_or("");
        let port = url.port_or_known_default();
        match &self.no_proxy {
            Some(no_proxy) if bypass_matches(no_proxy, host, port) => EnvDecision::Bypassed,
            _ => EnvDecision::UseProxy,
        }
    }
}

// ------------------------------------------------------------------ system

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemProxy {
    /// `host:port` for http:// requests.
    pub http: Option<String>,
    /// `host:port` for https:// requests.
    pub https: Option<String>,
    /// Auto-config script address (`AutoConfigURL` / `ProxyAutoConfigURLString`).
    pub pac_url: Option<String>,
    /// Exceptions (`ProxyOverride` / `ExceptionsList`).
    pub bypass: Vec<String>,
}

impl SystemProxy {
    fn bypasses(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        let port = url.port_or_known_default();
        self.bypass
            .iter()
            .any(|pattern| bypass_matches(pattern, host, port))
    }

    fn fixed_for(&self, url: &Url) -> Option<&String> {
        if url.scheme() == "https" {
            self.https.as_ref().or(self.http.as_ref())
        } else {
            self.http.as_ref().or(self.https.as_ref())
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.http.is_none() && self.https.is_none() && self.pac_url.is_none()
    }
}

/// `NO_PROXY` / `ProxyOverride` / `ExceptionsList` matching.
fn bypass_matches(list: &str, host: &str, port: Option<u16>) -> bool {
    let host = host.to_ascii_lowercase();
    list.split([';', ',']).any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        if entry.is_empty() {
            return false;
        }
        if entry == "*" {
            return true;
        }
        if entry == "<local>" {
            return !host.contains('.');
        }
        let (entry, entry_port) = match entry.rsplit_once(':') {
            Some((name, port_text)) => match port_text.parse::<u16>() {
                Ok(value) => (name.to_string(), Some(value)),
                Err(_) => (entry.clone(), None),
            },
            None => (entry.clone(), None),
        };
        if let (Some(wanted), Some(actual)) = (entry_port, port) {
            if wanted != actual {
                return false;
            }
        }
        if entry.contains('/') {
            // CIDR entries are not used for name-based matching.
            return false;
        }
        let entry = entry.trim_start_matches('.');
        if entry.is_empty() {
            return false;
        }
        if let Some(suffix) = entry.strip_prefix("*.") {
            return host == suffix || host.ends_with(&format!(".{suffix}"));
        }
        host == entry || host.ends_with(&format!(".{entry}"))
    })
}

// --------------------------------------------------------------- pure route

/// Decide the route from already-collected inputs (no IO), so the decision is
/// testable.
pub(crate) fn route_for(
    url: &Url,
    env: &ProxyEnv,
    system: &SystemProxy,
    pac_script: Option<&str>,
) -> Route {
    match env.decision(url) {
        EnvDecision::UseProxy => return Route::Environment,
        EnvDecision::Bypassed => {
            return Route::Direct {
                note: Some("NO_PROXY covers this host".to_string()),
            }
        }
        EnvDecision::NotConfigured => {}
    }
    if system.bypasses(url) {
        return Route::Direct {
            note: Some("the system proxy exceptions cover this host".to_string()),
        };
    }
    if let Some(pac_url) = &system.pac_url {
        let fallback = |reason: String| {
            match system
                .fixed_for(url)
                .and_then(|fixed| normalize_proxy(fixed).ok())
            {
                Some(url) => Route::Proxy {
                    url,
                    source: Source::SystemFixed,
                },
                None => Route::Direct { note: Some(reason) },
            }
        };
        let Some(script) = pac_script else {
            return fallback(format!("web_fetch could not read the PAC script {pac_url}"));
        };
        let host = url.host_str().unwrap_or("");
        let result = match super::web_fetch_pac::evaluate_pac(script, url.as_str(), host) {
            Ok(result) => result,
            Err(error) => {
                return fallback(format!(
                    "web_fetch could not evaluate PAC {pac_url}: {error}"
                ))
            }
        };
        return match pac_to_route(&result) {
            Ok(Some(proxy_url)) => Route::Proxy {
                url: proxy_url,
                source: Source::Pac,
            },
            Ok(None) => Route::Direct {
                note: Some(format!("the PAC script {pac_url} selected DIRECT")),
            },
            Err(reason) => Route::Unsupported {
                reason: format!("the PAC script {pac_url} selected {result:?}: {reason}"),
            },
        };
    }
    match system.fixed_for(url).map(|fixed| normalize_proxy(fixed)) {
        Some(Ok(url)) => Route::Proxy {
            url,
            source: Source::SystemFixed,
        },
        Some(Err(reason)) => Route::Unsupported { reason },
        None => Route::Direct { note: None },
    }
}

/// `host:port` / `http://host:port` → `http://host:port`; anything `reqwest`
/// cannot use becomes a reason, never a silent direct connection.
fn normalize_proxy(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("the configured proxy address is empty".to_string());
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&with_scheme)
        .map_err(|error| format!("the configured proxy `{raw}` is not a proxy URL: {error}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "the configured proxy `{raw}` uses the `{other}` scheme, which this build's HTTP client cannot use"
            ))
        }
    }
    if url.host_str().is_none() {
        return Err(format!("the configured proxy `{raw}` has no host"));
    }
    let mut out = url.to_string();
    while out.ends_with('/') {
        out.pop();
    }
    Ok(out)
}

/// PAC return value → the first usable directive.
fn pac_to_route(result: &str) -> Result<Option<String>, String> {
    let first = result
        .split(';')
        .map(str::trim)
        .find(|entry| !entry.is_empty())
        .unwrap_or("DIRECT");
    let mut parts = first.split_whitespace();
    let kind = parts.next().unwrap_or("").to_ascii_uppercase();
    let target = parts.next().unwrap_or("");
    match kind.as_str() {
        "DIRECT" => Ok(None),
        "PROXY" | "HTTP" | "HTTPS" => {
            if target.is_empty() {
                return Err("the PROXY directive has no host:port".to_string());
            }
            normalize_proxy(target).map(Some)
        }
        "SOCKS" | "SOCKS4" | "SOCKS5" => Err(
            "SOCKS proxies are not enabled in this build, so web_fetch cannot use them".to_string(),
        ),
        other => Err(format!(
            "`{other}` is not a proxy directive web_fetch understands"
        )),
    }
}

// ----------------------------------------------------------- system parsing

/// Parse `scutil --proxy` output (macOS).
pub(crate) fn parse_scutil_proxy(text: &str) -> SystemProxy {
    let mut out = SystemProxy::default();
    let mut http_enable = false;
    let mut https_enable = false;
    let mut pac_enable = false;
    let mut http_proxy = None;
    let mut http_port: Option<u16> = None;
    let mut https_proxy = None;
    let mut https_port: Option<u16> = None;
    let mut pac_url = None;
    let mut in_exceptions = false;
    for line in text.lines() {
        let line = line.trim();
        if in_exceptions {
            if line.starts_with('}') {
                in_exceptions = false;
                continue;
            }
            if let Some((_, value)) = line.split_once(':') {
                let value = value.trim();
                if !value.is_empty() {
                    out.bypass.push(value.to_string());
                }
            }
            continue;
        }
        if line.starts_with("ExceptionsList") {
            in_exceptions = true;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "HTTPEnable" => http_enable = value == "1",
            "HTTPProxy" => http_proxy = Some(value.to_string()),
            "HTTPPort" => http_port = value.parse().ok(),
            "HTTPSEnable" => https_enable = value == "1",
            "HTTPSProxy" => https_proxy = Some(value.to_string()),
            "HTTPSPort" => https_port = value.parse().ok(),
            "ProxyAutoConfigEnable" => pac_enable = value == "1",
            "ProxyAutoConfigURLString" => pac_url = Some(value.to_string()),
            _ => {}
        }
    }
    if http_enable {
        if let (Some(host), Some(port)) = (http_proxy.clone(), http_port) {
            out.http = Some(format!("{host}:{port}"));
        }
    }
    if https_enable {
        let host = https_proxy.clone().or(http_proxy);
        let port = https_port.or(http_port);
        if let (Some(host), Some(port)) = (host, port) {
            out.https = Some(format!("{host}:{port}"));
        }
    }
    if pac_enable {
        out.pac_url = pac_url.filter(|value| !value.is_empty());
    }
    out
}

/// Parse `reg query "…\Internet Settings"` output (Windows).
pub(crate) fn parse_windows_internet_settings(text: &str) -> SystemProxy {
    let mut enable = false;
    let mut server = None;
    let mut pac_url = None;
    let mut override_list = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(kind)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !kind.starts_with("REG_") {
            continue;
        }
        let value = parts.collect::<Vec<_>>().join(" ");
        if value.is_empty() {
            continue;
        }
        match name {
            "ProxyEnable" => enable = value != "0x0" && value != "0",
            "ProxyServer" => server = Some(value),
            "AutoConfigURL" => pac_url = Some(value),
            "ProxyOverride" => override_list = Some(value),
            _ => {}
        }
    }
    let mut out = SystemProxy::default();
    if enable {
        if let Some(server) = server {
            // `ProxyServer` is either `host:port` or per-scheme
            // `http=host:port;https=host:port`.
            if server.contains('=') {
                for entry in server.split(';') {
                    let Some((scheme, address)) = entry.split_once('=') else {
                        continue;
                    };
                    let address = address.trim().to_string();
                    if address.is_empty() {
                        continue;
                    }
                    match scheme.trim().to_ascii_lowercase().as_str() {
                        "http" => out.http = Some(address),
                        "https" => out.https = Some(address),
                        _ => {}
                    }
                }
            } else {
                out.http = Some(server.clone());
                out.https = Some(server);
            }
        }
    }
    out.pac_url = pac_url.filter(|value| !value.is_empty());
    if let Some(list) = override_list {
        out.bypass = list
            .split(';')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();
    }
    out
}

// ------------------------------------------------------------------ IO side

/// Read the operating system's proxy configuration. A missing or failing tool
/// yields the empty configuration, which means "no system proxy configured".
fn read_system_proxy() -> SystemProxy {
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("scutil")
            .arg("--proxy")
            .output()
        {
            Ok(output) if output.status.success() => {
                parse_scutil_proxy(&String::from_utf8_lossy(&output.stdout))
            }
            other => {
                tracing::debug!(
                    target: "codefactory::web_fetch",
                    "M195: `scutil --proxy` is unavailable: {:?}",
                    other.err()
                );
                SystemProxy::default()
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        match std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(output) if output.status.success() => {
                parse_windows_internet_settings(&String::from_utf8_lossy(&output.stdout))
            }
            other => {
                tracing::warn!(
                    target: "codefactory::web_fetch",
                    "M195: `reg query` for Internet Settings failed: {:?}",
                    other.err()
                );
                SystemProxy::default()
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        SystemProxy::default()
    }
}

/// Load a PAC script: a local path / `file://` URL, or an http(s) URL.
async fn load_pac(pac_url: &str) -> Option<String> {
    let url = pac_url.trim();
    if url.is_empty() {
        return None;
    }
    let path = if let Some(rest) = url.strip_prefix("file://") {
        Some(rest.to_string())
    } else if url.starts_with('/')
        || url.starts_with("\\\\")
        || url.as_bytes().get(1) == Some(&b':')
    {
        Some(url.to_string())
    } else {
        None
    };
    if let Some(path) = path {
        return match tokio::fs::read_to_string(&path).await {
            Ok(script) if script.len() <= MAX_PAC_BYTES => Some(script),
            Ok(_) => {
                tracing::warn!(
                    target: "codefactory::web_fetch",
                    "M195: the PAC script {path} is larger than {MAX_PAC_BYTES} bytes"
                );
                None
            }
            Err(error) => {
                tracing::warn!(
                    target: "codefactory::web_fetch",
                    "M195: could not read the PAC script {path}: {error}"
                );
                None
            }
        };
    }
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                target: "codefactory::web_fetch",
                "M195: could not build a client to fetch the PAC script: {error}"
            );
            return None;
        }
    };
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => {
            let bytes = response.bytes().await.ok()?;
            if bytes.len() > MAX_PAC_BYTES {
                tracing::warn!(
                    target: "codefactory::web_fetch",
                    "M195: the PAC script {url} is larger than {MAX_PAC_BYTES} bytes"
                );
                return None;
            }
            Some(String::from_utf8_lossy(&bytes).to_string())
        }
        Ok(response) => {
            tracing::warn!(
                target: "codefactory::web_fetch",
                "M195: the PAC script {url} returned {}",
                response.status()
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                target: "codefactory::web_fetch",
                "M195: could not fetch the PAC script {url}: {error}"
            );
            None
        }
    }
}

const MAX_PAC_BYTES: usize = 512 * 1024;

/// Decide how this URL should be fetched, reading the environment and the
/// operating system's proxy configuration. Pure decision, thin IO.
pub(crate) async fn select_route(url: &Url) -> Route {
    let env = ProxyEnv::from_process();
    if env.decision(url) == EnvDecision::UseProxy {
        return Route::Environment;
    }
    let system = read_system_proxy();
    if system.is_empty() {
        return route_for(url, &env, &system, None);
    }
    let script = match &system.pac_url {
        Some(pac_url) => load_pac(pac_url).await,
        None => None,
    };
    route_for(url, &env, &system, script.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("test url")
    }

    fn no_env() -> ProxyEnv {
        ProxyEnv::default()
    }

    /// Synthetic PAC in the shape corporate proxies actually ship.
    const CORPORATE_PAC: &str = r#"
// synthetic corporate PAC
function FindProxyForURL(url, host) {
  if (isPlainHostName(host) || shExpMatch(host, "*.corp.example")) {
    return "DIRECT";
  }
  if (dnsDomainIs(host, ".internal.example") || shExpMatch(host, "10.*")) {
    return "DIRECT";
  }
  return "PROXY proxy.corp.example:8080; DIRECT";
}
"#;

    fn pac_system() -> SystemProxy {
        SystemProxy {
            pac_url: Some("http://pac.corp.example/proxy.pac".to_string()),
            ..Default::default()
        }
    }

    // ---------------------------------------------------------------- R1: PAC

    #[test]
    fn m195_pac_routes_public_hosts_through_the_proxy() {
        let route = route_for(
            &url("https://metr.org/blog/2024"),
            &no_env(),
            &pac_system(),
            Some(CORPORATE_PAC),
        );
        assert_eq!(
            route,
            Route::Proxy {
                url: "http://proxy.corp.example:8080".to_string(),
                source: Source::Pac,
            }
        );
    }

    #[test]
    fn m195_pac_keeps_internal_hosts_direct() {
        for raw in [
            "https://wiki.internal.example/page",
            "http://buildbox/",
            "http://10.4.5.6/",
        ] {
            let route = route_for(&url(raw), &no_env(), &pac_system(), Some(CORPORATE_PAC));
            assert!(
                matches!(route, Route::Direct { .. }),
                "{raw} should stay direct, got {route:?}"
            );
        }
    }

    #[test]
    fn m195_environment_beats_the_system_pac() {
        let env = ProxyEnv {
            https_proxy: Some("http://env-proxy.example:3128".to_string()),
            ..Default::default()
        };
        assert_eq!(
            route_for(
                &url("https://metr.org/"),
                &env,
                &pac_system(),
                Some(CORPORATE_PAC)
            ),
            Route::Environment
        );
    }

    #[test]
    fn m195_no_proxy_excludes_a_host_from_the_system_pac_too() {
        let env = ProxyEnv {
            https_proxy: Some("http://env-proxy.example:3128".to_string()),
            no_proxy: Some(".metr.org,localhost".to_string()),
            ..Default::default()
        };
        let route = route_for(
            &url("https://metr.org/about"),
            &env,
            &pac_system(),
            Some(CORPORATE_PAC),
        );
        assert!(
            matches!(route, Route::Direct { .. }),
            "NO_PROXY must win over the PAC, got {route:?}"
        );
    }

    #[test]
    fn m195_system_bypass_list_keeps_a_host_direct() {
        let system = SystemProxy {
            pac_url: Some("http://pac.corp.example/proxy.pac".to_string()),
            bypass: vec!["*.corp.example".to_string()],
            ..Default::default()
        };
        let route = route_for(
            &url("https://wiki.corp.example/"),
            &no_env(),
            &system,
            Some(CORPORATE_PAC),
        );
        assert!(
            matches!(route, Route::Direct { .. }),
            "the system exceptions must win, got {route:?}"
        );
    }

    #[test]
    fn m195_unreadable_pac_falls_back_to_the_fixed_system_proxy() {
        let system = SystemProxy {
            https: Some("10.1.2.3:3128".to_string()),
            pac_url: Some("http://pac.corp.example/proxy.pac".to_string()),
            ..Default::default()
        };
        let route = route_for(&url("https://metr.org/"), &no_env(), &system, None);
        assert_eq!(
            route,
            Route::Proxy {
                url: "http://10.1.2.3:3128".to_string(),
                source: Source::SystemFixed,
            }
        );
    }

    #[test]
    fn m195_unreadable_pac_without_a_fixed_proxy_falls_back_to_direct() {
        let route = route_for(&url("https://metr.org/"), &no_env(), &pac_system(), None);
        match route {
            Route::Direct { note: Some(note) } => {
                assert!(note.contains("PAC"), "note should name the PAC: {note}")
            }
            other => panic!("expected a direct fallback with a note, got {other:?}"),
        }
    }

    #[test]
    fn m195_unsupported_pac_syntax_falls_back_instead_of_guessing() {
        let script = r#"function FindProxyForURL(url, host) {
            if (weekdayRange("SAT", "SUN")) { return "DIRECT"; }
            return "PROXY proxy.corp.example:8080";
        }"#;
        match route_for(&url("https://metr.org/"), &no_env(), &pac_system(), Some(script)) {
            Route::Direct { note: Some(note) } => {
                assert!(
                    note.contains("weekdayRange"),
                    "note should name the unsupported construct: {note}"
                )
            }
            other => panic!("expected a documented fallback, got {other:?}"),
        }
    }

    #[test]
    fn m195_socks_only_pac_is_reported_as_unsupported() {
        let script =
            r#"function FindProxyForURL(url, host) { return "SOCKS5 socks.example:1080"; }"#;
        match route_for(&url("https://metr.org/"), &no_env(), &pac_system(), Some(script)) {
            Route::Unsupported { reason } => {
                assert!(reason.contains("SOCKS"), "reason should name SOCKS: {reason}")
            }
            other => panic!("a SOCKS-only PAC must not silently go direct, got {other:?}"),
        }
    }

    // ------------------------------------------------------- R1: system parsing

    #[test]
    fn m195_macos_scutil_output_is_parsed() {
        // Synthetic `scutil --proxy` output with a PAC script configured.
        let text = "<dictionary> {\n\
                    \x20 ExceptionsList : <array> {\n\
                    \x20   0 : *.local\n\
                    \x20   1 : 169.254/16\n\
                    \x20 }\n\
                    \x20 HTTPEnable : 1\n\
                    \x20 HTTPPort : 6152\n\
                    \x20 HTTPProxy : 127.0.0.1\n\
                    \x20 HTTPSEnable : 1\n\
                    \x20 HTTPSPort : 6152\n\
                    \x20 HTTPSProxy : 127.0.0.1\n\
                    \x20 ProxyAutoConfigEnable : 1\n\
                    \x20 ProxyAutoConfigURLString : http://wpad.corp.example/proxy.pac\n\
                    }\n";
        let system = parse_scutil_proxy(text);
        assert_eq!(system.http.as_deref(), Some("127.0.0.1:6152"));
        assert_eq!(system.https.as_deref(), Some("127.0.0.1:6152"));
        assert_eq!(
            system.pac_url.as_deref(),
            Some("http://wpad.corp.example/proxy.pac")
        );
        assert_eq!(system.bypass, vec!["*.local", "169.254/16"]);
    }

    #[test]
    fn m195_macos_scutil_disabled_proxy_is_ignored() {
        let text = "<dictionary> {\n\
                    \x20 HTTPEnable : 0\n\
                    \x20 HTTPPort : 6152\n\
                    \x20 HTTPProxy : 127.0.0.1\n\
                    \x20 ProxyAutoConfigEnable : 0\n\
                    \x20 ProxyAutoConfigURLString : http://wpad.corp.example/proxy.pac\n\
                    }\n";
        let system = parse_scutil_proxy(text);
        assert_eq!(system.http, None);
        assert_eq!(system.pac_url, None);
        assert!(system.is_empty());
    }

    #[test]
    fn m195_windows_internet_settings_are_parsed() {
        // Synthetic `reg query` output: PAC script + fixed proxy + exceptions.
        let text = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\n\
                    \x20   AutoConfigURL    REG_SZ    http://pac.corp.example/proxy.pac\n\
                    \x20   ProxyEnable    REG_DWORD    0x1\n\
                    \x20   ProxyServer    REG_SZ    http://proxy.corp.example:8080\n\
                    \x20   ProxyOverride    REG_SZ    *.corp.example;<local>\n";
        let system = parse_windows_internet_settings(text);
        assert_eq!(
            system.pac_url.as_deref(),
            Some("http://pac.corp.example/proxy.pac")
        );
        assert_eq!(system.http.as_deref(), Some("http://proxy.corp.example:8080"));
        assert_eq!(
            system.https.as_deref(),
            Some("http://proxy.corp.example:8080")
        );
        assert_eq!(system.bypass, vec!["*.corp.example", "<local>"]);
    }

    #[test]
    fn m195_windows_per_scheme_proxy_is_parsed() {
        let text = "\x20   ProxyEnable    REG_DWORD    0x1\n\
                    \x20   ProxyServer    REG_SZ    http=proxy.corp.example:8080;https=secure.corp.example:8443\n";
        let system = parse_windows_internet_settings(text);
        assert_eq!(system.http.as_deref(), Some("proxy.corp.example:8080"));
        assert_eq!(
            system.https.as_deref(),
            Some("secure.corp.example:8443")
        );
        assert_eq!(system.pac_url, None);
    }

    #[test]
    fn m195_windows_disabled_proxy_is_ignored() {
        let text = "\x20   ProxyEnable    REG_DWORD    0x0\n\
                    \x20   ProxyServer    REG_SZ    proxy.corp.example:8080\n";
        let system = parse_windows_internet_settings(text);
        assert!(system.is_empty(), "disabled proxy must be ignored: {system:?}");
    }

    /// Real machine smoke test: the live macOS configuration must parse into a
    /// well-formed structure (no assertion about a proxy being present).
    #[cfg(target_os = "macos")]
    #[test]
    fn m195_real_macos_system_proxy_parses() {
        let system = read_system_proxy();
        for address in [system.http.as_deref(), system.https.as_deref()]
            .into_iter()
            .flatten()
        {
            assert!(
                address.rsplit_once(':').is_some(),
                "system proxy `{address}` should be host:port"
            );
        }
        if let Some(pac_url) = system.pac_url.as_deref() {
            assert!(!pac_url.is_empty(), "PAC URL should not be empty");
        }
    }
}
