// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Reasoning trace from "thinking-mode" models (DeepSeek reasoner family,
    /// Claude extended-thinking, etc.). When this field is present on an
    /// assistant message that's being replayed, the provider requires us
    /// to echo it back verbatim — omitting it triggers HTTP 400.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// OpenAI-compatible image content. When `type == "image_url"`, this
    /// carries `{"url": "data:image/png;base64,..."}`. OpenRouter, OpenAI,
    /// most providers honour this format directly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<ImageUrl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    pub stream: bool,
    pub temperature: f32,
    pub max_tokens: u32,
    /// OpenAI-standard streaming usage-reporting toggle. Supported by
    /// OpenRouter, DeepSeek, OpenAI, Together, Fireworks, etc. Avoid the
    /// OpenRouter-only `usage: { include: true }` field — providers like
    /// DeepSeek reject unknown top-level fields with HTTP 400.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub r#type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ── Streaming types ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: Delta,
    pub finish_reason: Option<String>,
    // Choice index from the wire (always 0 for our single-completion requests);
    // deserialized for wire fidelity, not consumed.
    #[allow(dead_code)]
    pub index: u32,
}

#[derive(Debug, Deserialize, Default)]
pub struct Delta {
    // Role marker ("assistant") sent on the first delta; we assume assistant, so
    // it's deserialized but unused.
    #[allow(dead_code)]
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    /// Streamed reasoning trace (DeepSeek `deepseek-reasoner` etc.).
    /// Accumulated separately from `content` and persisted/replayed on
    /// subsequent turns.
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    pub id: Option<String>,
    // Always "function" on the wire; deserialized but unused.
    #[allow(dead_code)]
    pub r#type: Option<String>,
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize, Default)]
pub struct FunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    // Provider's prompt+completion sum; we track the two components separately,
    // so the total is deserialized but unused.
    #[allow(dead_code)]
    pub total_tokens: u32,
    /// OpenRouter includes actual request cost in the final usage chunk.
    /// Direct providers and subscription endpoints commonly omit it.
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokenDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokenDetails>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PromptTokenDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}

#[derive(Debug, Deserialize, Default)]
pub struct CompletionTokenDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

// ── Model list ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub context_length: u32,
    #[serde(default)]
    pub pricing: Option<ModelPricing>,
    #[serde(default)]
    pub supported_parameters: Option<Vec<String>>,
    /// True when this entry came from `Endpoint.custom_models` rather than
    /// the remote `/models` API. The frontend uses this to label / highlight.
    #[serde(default)]
    pub is_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub prompt: String,
    pub completion: String,
}

#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<ModelInfo>,
}

// ── Events sent to frontend via Tauri emit ───────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta {
        content: String,
    },
    ToolCallStart {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolCallArgsDelta {
        index: u32,
        chunk: String,
    },
    ToolCallEnd {
        index: u32,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
        status: String,
    },
    PermissionRequest {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    Done {
        input_tokens: u32,
        output_tokens: u32,
    },
    /// Snapshot of how much of the model's context window the last prompt
    /// occupied. Emitted after every assistant turn (whenever we get a
    /// `prompt_tokens` reading back from the provider).
    ContextUsage {
        used_tokens: u32,
        limit_tokens: u32,
        max_limit_tokens: u32,
    },
    /// Notification that the older half of the conversation was compressed
    /// to fit the window. Frontend can toast this so the user knows why a
    /// previous tool result now shows "[elided]".
    ContextCompressed {
        elided_count: usize,
        tokens_freed: u32,
    },
    /// The provider request hit a transient transport or gateway failure and
    /// is being retried in-place instead of failing the user-visible turn.
    TransportRetry {
        label: String,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        reason: String,
    },
    /// The completion gate acted on this turn: `kind: "recovery"` — it
    /// rejected the model's tool-call-free final response and injected a
    /// recovery instruction (the turn continues); `kind: "ready"` — evidence
    /// is satisfied and a final coverage-audit instruction was injected.
    /// Surfaced so the user can see WHY the assistant keeps going instead of
    /// watching it silently repeat itself.
    CompletionGateAction {
        kind: String,
        detail: String,
    },
    Error {
        message: String,
    },
}
