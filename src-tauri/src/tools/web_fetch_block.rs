// SPDX-License-Identifier: Apache-2.0
//! M195 — recognise a corporate gateway's "blocked / risk" interstitial.
//!
//! On a filtered corporate network, `web_fetch` often gets a perfectly normal
//! `200 OK` whose body is the gateway's warning page instead of the article the
//! model asked for. Handing that to the model as content is misleading: it
//! looks like the site says "you are blocked by your network's security
//! policy". Detecting it lets `web_fetch` say what actually happened.
//!
//! Detection is a score, not a single keyword, so ordinary pages that merely
//! mention security policy are not misfiled:
//!
//! | signal | points |
//! |---|---|
//! | title carries a strong block-page phrase | 3 |
//! | body carries block-page phrases (1 each, max 3) | ≤3 |
//! | the request was redirected to a different host | 2 |
//! | the final host looks like a gateway | 1 |
//!
//! A body longer than [`LONG_PAGE_CHARS`] characters only counts when the score
//! reaches [`LONG_PAGE_SCORE`] — real articles quote "security policy" too.

use url::Url;

pub(crate) const LONG_PAGE_CHARS: usize = 5_000;
pub(crate) const LONG_PAGE_SCORE: u32 = 5;
pub(crate) const BLOCK_THRESHOLD: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockPage {
    /// Host that served the interstitial (the gateway).
    pub gateway_host: String,
    /// URL `web_fetch` was asked for.
    pub requested_url: String,
    /// URL that actually answered.
    pub final_url: String,
    pub evidence: Vec<String>,
}

impl BlockPage {
    pub(crate) fn error_code(&self) -> &'static str {
        "BLOCKED_BY_NETWORK_POLICY"
    }
}

/// Titles are the strongest single signal — a page titled "安全风险提示" is a
/// gateway page, not an article.
const TITLE_MARKERS: &[&str] = &[
    "安全风险提示",
    "风险提示",
    "安全警告",
    "安全提示",
    "网站已被拦截",
    "网页已被拦截",
    "已被拦截",
    "访问被拒绝",
    "上网行为管理",
    "网络安全提示",
    "web page blocked",
    "site blocked",
    "page blocked",
    "access blocked",
    "access denied",
    "security risk",
    "internet usage policy",
    "connection blocked",
    "restricted access",
];

/// Bodies need one specific phrase; a bare "blocked" is not enough.
const TEXT_MARKERS: &[&str] = &[
    "根据公司网络安全策略",
    "网络安全策略",
    "上网行为管理",
    "存在安全风险",
    "已被安全策略",
    "已被拦截",
    "该网站不安全",
    "网站已被屏蔽",
    "您的访问请求被拦截",
    "此页面被拦截",
    "本页面已被拦截",
    "your organization's security policy",
    "blocked by your organization",
    "this site has been blocked",
    "this page has been blocked",
    "web page blocked",
    "internet usage policy",
    "acceptable use policy",
    "the website you are trying to access",
    "has been blocked by",
    "sangfor",
    "fortiguard",
    "websense",
    "webwasher",
    "palo alto networks",
];

const GATEWAY_NAMES: &[&str] = &[
    "sangfor",
    "fortinet",
    "fortiguard",
    "websense",
    "webfilter",
    "blockpage",
    "block-page",
    "wpad",
];

const GATEWAY_PREFIXES: &[&str] = &[
    "gw.", "gateway.", "nac.", "portal.", "ise.", "proxy.", "auth.", "wifi.", "internet.",
];

fn looks_like_gateway_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    GATEWAY_NAMES.iter().any(|name| host.contains(name))
        || GATEWAY_PREFIXES.iter().any(|prefix| host.starts_with(prefix))
}

/// Decide whether this response is a network-policy interstitial.
pub(crate) fn detect(
    requested: &Url,
    final_url: &Url,
    status: u16,
    title: Option<&str>,
    text: &str,
) -> Option<BlockPage> {
    let title = title.unwrap_or("").to_ascii_lowercase();
    let text_lower = text.to_ascii_lowercase();
    let mut score = 0;
    let mut evidence = Vec::new();

    if let Some(marker) = TITLE_MARKERS.iter().find(|marker| title.contains(**marker)) {
        score += 3;
        evidence.push(format!("the page title contains {marker:?}"));
    }

    let hits: Vec<&&str> = TEXT_MARKERS
        .iter()
        .filter(|marker| text_lower.contains(**marker))
        .collect();
    if !hits.is_empty() {
        let counted = hits.len().min(3) as u32;
        score += counted;
        let names: Vec<&str> = hits.iter().take(3).map(|marker| **marker).collect();
        evidence.push(format!("the page body mentions {names:?}"));
    }

    let requested_host = requested.host_str().unwrap_or("").to_ascii_lowercase();
    let final_host = final_url.host_str().unwrap_or("").to_ascii_lowercase();
    if requested_host != final_host {
        score += 2;
        evidence.push(format!(
            "{requested_host} was redirected to a different host ({final_host})"
        ));
    }
    if looks_like_gateway_host(&final_host) {
        score += 1;
        evidence.push(format!("{final_host} looks like a network gateway"));
    }

    if status == 407 {
        // A proxy demanding authentication is not an interstitial, it is its
        // own explicit error; do not classify it here.
        return None;
    }

    let long_page = text.chars().count() > LONG_PAGE_CHARS;
    let threshold = if long_page {
        LONG_PAGE_SCORE
    } else {
        BLOCK_THRESHOLD
    };
    if score < threshold {
        return None;
    }
    Some(BlockPage {
        gateway_host: final_host,
        requested_url: requested.to_string(),
        final_url: final_url.to_string(),
        evidence,
    })
}

/// The structured error the model sees. It explains the situation and offers
/// only policy-respecting options (M195 R3): another public source, or the
/// user's own browser session — which is subject to the same policy.
pub(crate) fn message(block: &BlockPage) -> String {
    format!(
        "{} — web_fetch asked for {requested} but the network answered with a block page instead of the page.\n\
         Gateway host: {gateway}\n\
         Final URL: {final}\n\
         Why: {evidence}\n\
         This request was stopped by your network's security policy (gateway / proxy filtering), not by the site \
         and not by web_fetch. web_fetch does not try to get around that policy.\n\
         What you can do:\n\
         - Use another public source for the same information (the project's own site, a mirror, a different domain).\n\
         - If the page opens in the user's own browser, it can still be read with the `browser_session` tool: \
         action=\"attach\", select the tab with this URL, then action=\"read\". The same network policy applies there.",
        block.error_code(),
        requested = block.requested_url,
        gateway = if block.gateway_host.is_empty() {
            "unknown"
        } else {
            block.gateway_host.as_str()
        },
        final = block.final_url,
        evidence = block.evidence.join("; "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("test url")
    }

    /// A real-shape Chinese gateway interstitial: 200 OK, risk title, policy text.
    const GATEWAY_TITLE: &str = "安全风险提示";
    const GATEWAY_BODY: &str = "风险提示：根据公司网络安全策略，您访问的网站存在安全风险，已被拦截。\
         如需访问请联系 IT 服务台。";

    #[test]
    fn m195_gateway_page_with_a_risk_title_is_a_block_page() {
        let block = detect(
            &url("https://metr.org/blog/2024"),
            &url("https://metr.org/blog/2024"),
            200,
            Some(GATEWAY_TITLE),
            GATEWAY_BODY,
        )
        .expect("a risk-titled page must be recognised as a block page");
        assert_eq!(block.gateway_host, "metr.org");
        assert_eq!(block.requested_url, "https://metr.org/blog/2024");
        let message = message(&block);
        assert!(message.starts_with("BLOCKED_BY_NETWORK_POLICY"));
        assert!(message.contains("https://metr.org/blog/2024"));
        assert!(message.contains("browser_session"));
    }

    #[test]
    fn m195_redirect_to_a_gateway_host_is_a_block_page() {
        let block = detect(
            &url("https://metr.org/blog/2024"),
            &url("http://gw.corp.example/block?url=metr.org"),
            200,
            Some("Access Denied"),
            "The website you are trying to access has been blocked by your organization's security policy.",
        )
        .expect("a redirect to a gateway is a block page");
        assert_eq!(block.gateway_host, "gw.corp.example");
        assert!(block.evidence.iter().any(|item| item.contains("redirected")));
        let message = message(&block);
        assert!(message.contains("gw.corp.example"));
        assert!(message.contains("https://metr.org/blog/2024"));
    }

    #[test]
    fn m195_unknown_gateway_page_is_caught_by_body_phrases_alone() {
        let block = detect(
            &url("https://example.org/report"),
            &url("https://example.org/report"),
            200,
            Some("Notice"),
            "您的访问请求被拦截：根据公司网络安全策略，该网站不安全。",
        )
        .expect("policy phrases in the body are enough");
        assert!(block.evidence.iter().any(|item| item.contains("body")));
    }

    #[test]
    fn m195_ordinary_article_is_not_a_block_page() {
        let article = format!(
            "{} This post discusses how a security policy shapes engineering decisions, \
             and why the term 'blocked' shows up in incident reports so often. {}",
            "Security work is mostly about clarity. ".repeat(120),
            "Read the rest for context. ".repeat(120)
        );
        assert!(article.chars().count() > LONG_PAGE_CHARS);
        let detected = detect(
            &url("https://metr.org/blog/security-policy"),
            &url("https://metr.org/blog/security-policy"),
            200,
            Some("How security policy shapes engineering"),
            &article,
        );
        assert!(
            detected.is_none(),
            "an ordinary article must not be mistaken for a block page: {detected:?}"
        );
    }

    #[test]
    fn m195_long_page_needs_more_evidence_than_a_short_one() {
        // Title (3) + one body phrase (1) = 4: enough for a tiny interstitial,
        // not enough for a long page that merely talks about policy.
        let long = format!(
            "{}{}",
            "网络安全策略 ".repeat(900),
            "其余内容请参考文档。".repeat(20)
        );
        assert!(long.chars().count() > LONG_PAGE_CHARS);
        assert!(
            detect(
                &url("https://example.org/policy"),
                &url("https://example.org/policy"),
                200,
                Some("Access Denied"),
                &long
            )
            .is_none(),
            "a long page must clear the higher threshold"
        );
        assert!(
            detect(
                &url("https://example.org/policy"),
                &url("https://example.org/policy"),
                200,
                Some("Access Denied"),
                "网络安全策略"
            )
            .is_some(),
            "the same evidence on a short page is a block page"
        );
    }

    #[test]
    fn m195_short_page_mentioning_a_policy_phrase_once_is_not_a_block_page() {
        let detected = detect(
            &url("https://example.org/docs"),
            &url("https://example.org/docs"),
            200,
            Some("Deployment notes"),
            "Our deploy script follows the company security policy for signing keys.",
        );
        assert!(detected.is_none(), "a single weak phrase is not enough");
    }

    #[test]
    fn m195_proxy_authentication_is_not_a_block_page() {
        assert!(detect(
            &url("https://metr.org/"),
            &url("https://metr.org/"),
            407,
            Some("Proxy Authentication Required"),
            "Access Denied",
        )
        .is_none());
    }

    #[test]
    fn m195_advice_never_suggests_bypassing_the_policy() {
        let block = detect(
            &url("https://metr.org/"),
            &url("http://gw.corp.example/block"),
            200,
            Some("安全风险提示"),
            GATEWAY_BODY,
        )
        .expect("block page");
        let message = message(&block).to_ascii_lowercase();
        for forbidden in ["dns", "spoof", "user-agent", "vpn", "proxy server:", "direct connect"] {
            assert!(
                !message.contains(forbidden),
                "the advice must not suggest bypassing the policy, but mentions `{forbidden}`"
            );
        }
    }
}
