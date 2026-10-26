// SPDX-License-Identifier: Apache-2.0
//! 视觉桥：让不支持图像输入的模型也能"看图"。
//!
//! 两条互不依赖的路径：
//! 1. 视觉助手路由 —— 把图片以 OpenAI 兼容的 `image_url` 内容块发给一个明确
//!    支持视觉的端点/模型，取回纯文本描述，并在输出里注明"图片已发送给该端点"；
//! 2. 本地 OCR 兜底 —— 没有可用视觉端点时，用系统 OCR 提取文字与文字位置，
//!    并在输出里明确标注这是降级结果。
//!
//! 两条路径都不让回合失败：真正拿不到任何信息时返回可读的说明文本。
//!
//! 结果缓存以图片内容哈希为键，命中后不再产生任何模型调用。

use std::future::Future;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::agent::failover::RouteCandidate;
use crate::config::settings::{ApiStyle, Settings};

pub mod ocr;
pub mod render;

/// 用量归属标签。视觉桥的模型调用以此与普通对话回合区分。
pub const SURFACE: &str = "vision_bridge";
/// 送入单张图片的上限，超过即拒绝，避免把整段请求顶爆。
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// 视觉助手单次描述的时限。
pub const MODEL_DEADLINE: Duration = Duration::from_secs(45);

/// 一个图片输入：原始字节 + MIME + 来源标签（文件名或"某文档第 N 页"）。
#[derive(Debug, Clone)]
pub struct ImageInput {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub label: String,
}

impl ImageInput {
    pub fn new(bytes: Vec<u8>, mime: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            bytes,
            mime: mime.into(),
            label: label.into(),
        }
    }

    /// 内容哈希。同一张图重复提问会命中同一条缓存。
    pub fn sha256(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.bytes);
        format!("{:x}", hasher.finalize())
    }

    /// OpenAI 兼容的 `data:` URL，供 `image_url` 内容块使用。
    pub fn data_url(&self) -> String {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.bytes);
        format!("data:{};base64,{}", self.mime, encoded)
    }
}

/// 一次看图请求实际走了哪条路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// 图片发给了支持视觉的模型端点。
    VisionModel,
    /// 本地 OCR（未把图片发给任何模型）。
    LocalOcr,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VisionModel => "vision_model",
            Self::LocalOcr => "local_ocr",
        }
    }
}

/// 看图结果。`text` 是给模型读的正文，其余字段用于归属与降级说明。
#[derive(Debug, Clone)]
pub struct VisionAnswer {
    pub text: String,
    pub backend: Backend,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    /// 走的哪个本地 OCR 后端（仅 `LocalOcr`）。
    pub ocr_backend: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// 必须原样出现在工具输出里的降级说明（例如"仅识别文字，没有版面描述"）。
    pub degraded_note: Option<String>,
    /// 本地 OCR 拿到的文字块位置，用于回答"左上角写了什么"这类问题。
    pub words: Vec<ocr::OcrWord>,
}

impl VisionAnswer {
    fn model_answer(
        text: String,
        route: &RouteCandidate,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Self {
        Self {
            text,
            backend: Backend::VisionModel,
            endpoint: Some(route.endpoint_name.clone()),
            model: Some(route.model_id.clone()),
            ocr_backend: None,
            input_tokens,
            output_tokens,
            degraded_note: None,
            words: Vec::new(),
        }
    }

    /// 渲染成工具输出正文。首行永远是归属说明，模型能读到"这张图去了哪里"。
    pub fn render(&self, image: &ImageInput, question: &str) -> String {
        let mut out = String::new();
        match self.backend {
            Backend::VisionModel => {
                out.push_str(&format!(
                    "视觉助手：{} / {}（图片已发送给该端点识别）\n",
                    self.endpoint.as_deref().unwrap_or("?"),
                    self.model.as_deref().unwrap_or("?")
                ));
            }
            Backend::LocalOcr => {
                out.push_str(&format!(
                    "本地 OCR（{}）：未使用视觉模型，图片没有发送给任何端点\n",
                    self.ocr_backend.as_deref().unwrap_or("unknown")
                ));
            }
        }
        out.push_str(&format!(
            "图片：{}（{} 字节）\n",
            image.label,
            image.bytes.len()
        ));
        out.push_str(&format!("问题：{question}\n\n"));
        if let Some(note) = self.degraded_note.as_deref() {
            out.push_str(&format!("注意：{note}\n\n"));
        }
        out.push_str(self.text.trim());
        if !self.words.is_empty() {
            out.push_str("\n\n文字块位置（归一化 0-1，左上为原点）：\n");
            for word in self.words.iter().take(200) {
                out.push_str(&format!(
                    "  {}  x={:.3} y={:.3} w={:.3} h={:.3}\n",
                    word.text, word.x, word.y, word.width, word.height
                ));
            }
        }
        out
    }
}

/// 可用的视觉助手路由，按"用户指定 > 当前默认端点 > 其余端点"排序。
///
/// 只收明确支持视觉、且确实有凭据可用的路由：ChatGPT 订阅走内置鉴权，
/// 其余端点需要 `key_ref`。
pub fn vision_routes(settings: &Settings) -> Vec<RouteCandidate> {
    let mut routes: Vec<(u8, RouteCandidate)> = Vec::new();
    let preferred = settings
        .vision_assistant
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());

    for (name, endpoint) in &settings.endpoints {
        let mut models: Vec<String> = endpoint
            .custom_models
            .iter()
            .map(|m| m.id.clone())
            .collect();
        if let Some(active) = endpoint.active_model.as_ref() {
            if !models.iter().any(|m| m == active) {
                models.push(active.clone());
            }
        }
        for model in models {
            if !crate::agent::context::model_supports_vision(settings, name, &model) {
                continue;
            }
            let mut rank = 2;
            if settings.default_endpoint == *name {
                rank = 1;
            }
            if let Some(pref) = preferred {
                if preference_matches(pref, name, &model) {
                    rank = 0;
                }
            }
            routes.push((
                rank,
                RouteCandidate {
                    endpoint_name: name.clone(),
                    model_id: model,
                    base_url: endpoint.base_url.clone(),
                    credential_ref: endpoint.key_ref.clone(),
                    legacy_inline_api_key: None,
                    supports_vision: true,
                    api_style: endpoint.api_style.clone(),
                },
            ));
        }
    }

    // 稳定排序：rank 升序，其次端点名、模型名，保证测试与真实选择一致。
    routes.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.endpoint_name.cmp(&b.1.endpoint_name))
            .then_with(|| a.1.model_id.cmp(&b.1.model_id))
    });
    routes
        .into_iter()
        .filter(|(_, route)| route_is_usable(route))
        .map(|(_, route)| route)
        .collect()
}

fn preference_matches(pref: &str, endpoint: &str, model: &str) -> bool {
    if let Some((want_endpoint, want_model)) = pref.split_once("::") {
        return want_endpoint.trim() == endpoint && want_model.trim() == model;
    }
    pref == model || pref == format!("{endpoint}/{model}")
}

fn route_is_usable(route: &RouteCandidate) -> bool {
    matches!(route.api_style, ApiStyle::Chatgpt)
        || route.credential_ref.is_some()
        || route.legacy_inline_api_key.is_some()
}

/// 真正调用视觉端点。图片以 `image_url` 内容块随问题一起发送。
pub async fn ask_vision_route(
    route: RouteCandidate,
    session_id: &str,
    image: &ImageInput,
    question: &str,
) -> Result<VisionAnswer, String> {
    use crate::openrouter::types::{ChatMessage, ContentPart, ImageUrl, MessageContent};

    if image.bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "IMAGE_TOO_LARGE: {} 字节超过上限 {} 字节",
            image.bytes.len(),
            MAX_IMAGE_BYTES
        ));
    }

    let prompt = format!(
        "请阅读这张图片（{}）并回答问题。只回答图片里能看到的内容，看不到就直说。\n问题：{}",
        image.label, question
    );
    let message = ChatMessage {
        role: "user".into(),
        content: MessageContent::Parts(vec![
            ContentPart {
                r#type: "text".into(),
                text: Some(prompt),
                image_url: None,
            },
            ContentPart {
                r#type: "image_url".into(),
                text: None,
                image_url: Some(ImageUrl {
                    url: image.data_url(),
                }),
            },
        ]),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning_content: None,
    };

    let out = crate::agent::internal_text::generate_bounded_text(
        route.clone(),
        session_id,
        vec![message],
        1200,
        MODEL_DEADLINE,
    )
    .await?;

    // thinking 路由可能把答案整段放进 reasoning。
    let text = if out.text.trim().is_empty() {
        out.reasoning.unwrap_or_default()
    } else {
        out.text
    };
    if text.trim().is_empty() {
        return Err("VISION_EMPTY_ANSWER: 视觉端点没有返回任何文本".to_string());
    }
    let (input_tokens, output_tokens) = out
        .usage
        .as_ref()
        .map(|usage| (usage.prompt_tokens as u64, usage.completion_tokens as u64))
        .unwrap_or((0, 0));
    Ok(VisionAnswer::model_answer(
        text,
        &route,
        input_tokens,
        output_tokens,
    ))
}

/// 视觉桥缓存表。以（会话, 图片哈希, 问题哈希）为键。
async fn ensure_cache_table(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS vision_descriptions (
            session_id TEXT NOT NULL,
            image_sha TEXT NOT NULL,
            question_sha TEXT NOT NULL,
            backend TEXT NOT NULL,
            endpoint TEXT,
            model TEXT,
            text TEXT NOT NULL,
            degraded_note TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (session_id, image_sha, question_sha)
        )",
    )
    .execute(pool)
    .await
    .map_err(|error| format!("VISION_CACHE_UNAVAILABLE: {error}"))?;
    Ok(())
}

fn question_sha(question: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(question.trim().to_lowercase().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 带缓存的看图：命中缓存时**不会**调用 `call`。
///
/// 返回 `(答案, 是否命中缓存)`。缓存不可用时退化为直连调用，不报错。
pub async fn describe_cached<F, Fut>(
    db: Option<&SqlitePool>,
    session_id: &str,
    image: &ImageInput,
    question: &str,
    call: F,
) -> Result<(VisionAnswer, bool), String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<VisionAnswer, String>>,
{
    let image_sha = image.sha256();
    let question_sha = question_sha(question);

    if let Some(pool) = db {
        if ensure_cache_table(pool).await.is_ok() {
            let row: Option<(
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
            )> = sqlx::query_as(
                "SELECT backend, endpoint, model, text, degraded_note
                     FROM vision_descriptions
                     WHERE session_id = ? AND image_sha = ? AND question_sha = ?",
            )
            .bind(session_id)
            .bind(&image_sha)
            .bind(&question_sha)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
            if let Some((backend, endpoint, model, text, degraded_note)) = row {
                let backend = if backend == Backend::LocalOcr.as_str() {
                    Backend::LocalOcr
                } else {
                    Backend::VisionModel
                };
                return Ok((
                    VisionAnswer {
                        text,
                        backend,
                        endpoint,
                        model,
                        ocr_backend: None,
                        input_tokens: 0,
                        output_tokens: 0,
                        degraded_note,
                        words: Vec::new(),
                    },
                    true,
                ));
            }
        }
    }

    let answer = call().await?;

    if let Some(pool) = db {
        if ensure_cache_table(pool).await.is_ok() {
            let _ = sqlx::query(
                "INSERT OR REPLACE INTO vision_descriptions
                 (session_id, image_sha, question_sha, backend, endpoint, model, text, degraded_note)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(session_id)
            .bind(&image_sha)
            .bind(&question_sha)
            .bind(answer.backend.as_str())
            .bind(answer.endpoint.as_deref())
            .bind(answer.model.as_deref())
            .bind(&answer.text)
            .bind(answer.degraded_note.as_deref())
            .execute(pool)
            .await;
        }
    }

    Ok((answer, false))
}

/// 本地 OCR 兜底结果包装成 `VisionAnswer`，降级说明是必填项。
pub fn local_ocr_answer(result: ocr::OcrResult, question: &str) -> VisionAnswer {
    let text = if result.text.trim().is_empty() {
        format!(
            "本地 OCR（{}）没有识别到文字。图片可能是纯图形；当前没有可用的视觉端点，无法描述画面内容。\n问题：{}",
            result.backend,
            question
        )
    } else {
        result.text.clone()
    };
    VisionAnswer {
        text,
        backend: Backend::LocalOcr,
        endpoint: None,
        model: None,
        ocr_backend: Some(result.backend.clone()),
        input_tokens: 0,
        output_tokens: 0,
        degraded_note: Some(format!(
            "这是本地 OCR 结果（{}）：只包含识别出的文字，没有画面描述，也没有把图片发送给任何模型。",
            result.backend
        )),
        words: result.words,
    }
}

/// 两条路径都没有时的最终说明。回合不因此失败。
pub fn no_backend_answer(
    image: &ImageInput,
    question: &str,
    ocr_error: Option<String>,
) -> VisionAnswer {
    let detail = ocr_error
        .map(|error| format!("\n本地 OCR 失败原因：{error}"))
        .unwrap_or_default();
    VisionAnswer {
        text: format!(
            "没有任何可用的看图路径：当前没有支持视觉的端点可用，本地 OCR 也不可用。\n图片：{}（{} 字节）\n问题：{question}{detail}\n\n建议：在设置里给某个端点勾选视觉能力，或安装 tesseract。",
            image.label,
            image.bytes.len()
        ),
        backend: Backend::LocalOcr,
        endpoint: None,
        model: None,
        ocr_backend: None,
        input_tokens: 0,
        output_tokens: 0,
        degraded_note: Some("本次没有读到图片内容。".to_string()),
        words: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(bytes: &[u8]) -> ImageInput {
        ImageInput::new(bytes.to_vec(), "image/png", "fixture.png")
    }

    #[test]
    fn image_sha_is_content_addressed() {
        assert_eq!(png(b"a").sha256(), png(b"a").sha256());
        assert_ne!(png(b"a").sha256(), png(b"b").sha256());
    }

    #[test]
    fn data_url_carries_mime_and_base64() {
        let url = png(b"abc").data_url();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        assert_eq!(url.trim_end(), "data:image/png;base64,YWJj");
    }

    #[test]
    fn local_ocr_answer_always_declares_degradation() {
        let answer = local_ocr_answer(
            ocr::OcrResult {
                backend: "tesseract".into(),
                text: "左上角文字".into(),
                words: vec![ocr::OcrWord {
                    text: "左上角文字".into(),
                    x: 0.1,
                    y: 0.2,
                    width: 0.3,
                    height: 0.04,
                }],
            },
            "左上角写了什么",
        );
        let rendered = answer.render(&png(b"x"), "左上角写了什么");
        assert!(rendered.contains("本地 OCR（tesseract）"));
        assert!(rendered.contains("未使用视觉模型"));
        assert!(rendered.contains("注意：这是本地 OCR 结果"));
        assert!(rendered.contains("x=0.100"));
    }

    #[test]
    fn vision_model_rendered_output_names_the_endpoint() {
        let route = RouteCandidate {
            endpoint_name: "vision-ep".into(),
            model_id: "vl-model".into(),
            base_url: "http://127.0.0.1:1".into(),
            credential_ref: None,
            legacy_inline_api_key: Some("k".into()),
            supports_vision: true,
            api_style: ApiStyle::Openai,
        };
        let answer = VisionAnswer::model_answer("图里是一只猫".into(), &route, 10, 5);
        let rendered = answer.render(&png(b"x"), "图里是什么");
        assert!(rendered.contains("视觉助手：vision-ep / vl-model"));
        assert!(rendered.contains("图片已发送给该端点识别"));
    }

    #[test]
    fn preference_and_default_ordering_are_stable() {
        use crate::config::settings::Endpoint;
        fn endpoint(name: &str, model: &str) -> Endpoint {
            Endpoint {
                base_url: format!("http://{name}"),
                key_ref: Some(format!("{name}-key")),
                api_style: ApiStyle::Openai,
                custom_models: Vec::new(),
                active_model: Some(model.into()),
            }
        }
        let mut settings = Settings::default();
        // 默认设置里带一个真实端点（claude 系列确实支持视觉），测试要隔离它。
        settings.endpoints.clear();
        settings.default_endpoint = "c-endpoint".into();
        settings.vision_assistant = Some("a-endpoint::a-vl".into());
        settings
            .endpoints
            .insert("a-endpoint".into(), endpoint("a-endpoint", "a-vl"));
        settings
            .endpoints
            .insert("c-endpoint".into(), endpoint("c-endpoint", "c-vl"));
        // 纯文本模型（deepseek 无 vl）必须被排除：它不能当视觉助手。
        settings
            .endpoints
            .insert("b-endpoint".into(), endpoint("b-endpoint", "deepseek-chat"));

        let routes = vision_routes(&settings);
        let names: Vec<(String, String)> = routes
            .iter()
            .map(|r| (r.endpoint_name.clone(), r.model_id.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("a-endpoint".to_string(), "a-vl".to_string()),
                ("c-endpoint".to_string(), "c-vl".to_string()),
            ],
            "preference first, then default endpoint; non-vision models excluded"
        );

        // 没有凭据又不是 ChatGPT 订阅的路由不可用。
        settings.endpoints.get_mut("c-endpoint").unwrap().key_ref = None;
        let routes = vision_routes(&settings);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].endpoint_name, "a-endpoint");
    }

    #[tokio::test]
    async fn cache_hit_does_not_call_the_backend_twice() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let mut calls = 0;
        let image = png(b"same-bytes");
        let answer = |calls: &mut i32| {
            *calls += 1;
            async {
                Ok::<VisionAnswer, String>(VisionAnswer {
                    text: "cached text".into(),
                    backend: Backend::VisionModel,
                    endpoint: Some("ep".into()),
                    model: Some("m".into()),
                    ocr_backend: None,
                    input_tokens: 1,
                    output_tokens: 1,
                    degraded_note: None,
                    words: Vec::new(),
                })
            }
        };

        let (first, hit1) =
            describe_cached(Some(&pool), "s1", &image, "问题", || answer(&mut calls))
                .await
                .unwrap();
        let (second, hit2) =
            describe_cached(Some(&pool), "s1", &image, "问题", || answer(&mut calls))
                .await
                .unwrap();

        assert!(!hit1);
        assert!(hit2, "second identical request must hit the cache");
        assert_eq!(calls, 1, "cache hit must not call the vision backend again");
        assert_eq!(first.text, second.text);
        assert_eq!(second.model.as_deref(), Some("m"));

        // 换一个问题必须重新调用：缓存键包含问题。
        let (_third, hit3) = describe_cached(Some(&pool), "s1", &image, "另一个问题", || {
            answer(&mut calls)
        })
        .await
        .unwrap();
        assert!(!hit3);
        assert_eq!(calls, 2);
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// 起一个只回一次话的 OpenAI 兼容服务，把收到的请求原文留在 `sink`。
    async fn stub_server(sink: Arc<Mutex<String>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = vec![0u8; 16 * 1024];
            // 读到 headers 完整且 body 满足 Content-Length 为止（上限 1 MiB）。
            loop {
                let read = match tokio::io::AsyncReadExt::read(&mut socket, &mut chunk).await {
                    Ok(read) => read,
                    Err(_) => break,
                };
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&buffer).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let header_len = split + 4;
                    let length = text[..split]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buffer.len() >= header_len + length {
                        break;
                    }
                }
                if buffer.len() > 1024 * 1024 {
                    break;
                }
            }
            *sink.lock().unwrap() = String::from_utf8_lossy(&buffer).to_string();

            // 应用侧走流式读取（SSE），桩服务必须按 SSE 回话。
            let chunk = serde_json::json!({
                "id": "stub",
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": "图里是一只橘猫" },
                    "finish_reason": null
                }]
            })
            .to_string();
            let tail = serde_json::json!({
                "id": "stub",
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 }
            })
            .to_string();
            let payload = format!("data: {chunk}\n\ndata: {tail}\n\ndata: [DONE]\n\n");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
        });
        format!("http://{address}/v1")
    }

    #[tokio::test]
    async fn image_is_sent_as_an_image_url_part_and_the_answer_comes_back() {
        let sink = Arc::new(Mutex::new(String::new()));
        let base_url = stub_server(sink.clone()).await;
        let route = RouteCandidate {
            endpoint_name: "stub".into(),
            model_id: "stub-vl".into(),
            base_url,
            credential_ref: None,
            legacy_inline_api_key: Some("test-key".into()),
            supports_vision: true,
            api_style: ApiStyle::Openai,
        };
        let image = ImageInput::new(b"PNGDATA".to_vec(), "image/png", "shot.png");

        let answer = ask_vision_route(route, "session-wire", &image, "图里是什么")
            .await
            .expect("the stub must answer");
        assert_eq!(answer.text, "图里是一只橘猫");
        assert_eq!(answer.backend, Backend::VisionModel);
        assert_eq!(answer.model.as_deref(), Some("stub-vl"));
        assert_eq!(answer.input_tokens, 11);
        assert_eq!(answer.output_tokens, 7);

        let request = sink.lock().unwrap().clone();
        assert!(
            request.starts_with("POST /v1/chat/completions"),
            "wrong route: {}",
            request.lines().next().unwrap_or_default()
        );
        assert!(
            request.contains("\"type\":\"image_url\""),
            "the image part never reached the endpoint: {request}"
        );
        assert!(
            request.contains("data:image/png;base64,UE5HREFUQQ=="),
            "the image bytes were not carried as a data URL: {request}"
        );
        assert!(
            request.contains("图里是什么"),
            "the user question was dropped: {request}"
        );
        assert!(
            request.contains("stub-vl"),
            "the chosen model was not requested: {request}"
        );
    }

    #[tokio::test]
    async fn oversized_images_are_refused_before_any_request() {
        let route = RouteCandidate {
            endpoint_name: "stub".into(),
            model_id: "stub-vl".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            credential_ref: None,
            legacy_inline_api_key: Some("test-key".into()),
            supports_vision: true,
            api_style: ApiStyle::Openai,
        };
        let image = ImageInput::new(vec![0u8; MAX_IMAGE_BYTES + 1], "image/png", "huge.png");
        let error = ask_vision_route(route, "s", &image, "q").await.unwrap_err();
        assert!(error.starts_with("IMAGE_TOO_LARGE"), "{error}");
    }
}
