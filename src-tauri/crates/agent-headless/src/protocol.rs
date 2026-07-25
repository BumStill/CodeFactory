// SPDX-License-Identifier: Apache-2.0
//! JSONL wire protocol: the message types and the stdin/stdout reader/writer.
//!
//! Extracted verbatim from `main.rs` (keystone slice 4.8a) — a pure module
//! split with ZERO behaviour change, so the later seam adoption (4.8b) shows up
//! as a small readable diff instead of being buried in a 2775-line file.


use codefactory_agent_core::*;
use crate::{HeadlessError, Usage};
use crate::policy::*;
use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum InputMessage {
    #[serde(rename = "start")]
    Start {
        instruction: String,
        model: String,
        api_key: String,
        base_url: String,
        max_steps: u32,
        model_timeout_sec: u64,
        shell_timeout_sec: u64,
        #[serde(default)]
        wall_time_budget_sec: Option<u64>,
        #[serde(default)]
        working_directory: Option<String>,
        allow_network: bool,
        #[serde(default)]
        policy_profile: RuntimePolicyProfile,
        execution_contract_sha256: String,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        id: String,
        return_code: Option<i32>,
        stdout: String,
        stderr: String,
        error: Option<String>,
        #[serde(default)]
        next_working_directory: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub(crate) enum OutputMessage {
    #[serde(rename = "tool_request")]
    ToolRequest {
        id: String,
        command: String,
        timeout_sec: u64,
        usage: Usage,
    },
    #[serde(rename = "event")]
    UsageSnapshot { name: String, usage: Usage },
    #[serde(rename = "finished")]
    Finished {
        final_text: String,
        execution_contract_sha256: String,
        completion_evidence: CompletionEvidence,
        usage: Usage,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct StartConfig {
    pub(crate) instruction: String,
    pub(crate) model: String,
    pub(crate) api_key: String,
    pub(crate) base_url: String,
    pub(crate) max_steps: u32,
    pub(crate) model_timeout_sec: u64,
    pub(crate) shell_timeout_sec: u64,
    pub(crate) wall_time_budget_sec: Option<u64>,
    pub(crate) working_directory: Option<String>,
    pub(crate) allow_network: bool,
    pub(crate) policy_profile: RuntimePolicyProfile,
}

pub(crate) async fn read_start<R>(input: &mut R) -> Result<StartConfig, HeadlessError>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_protocol_line(input)
        .await?
        .ok_or(HeadlessError::MissingStart)?;
    match serde_json::from_str::<InputMessage>(&line)? {
        InputMessage::Start {
            instruction,
            model,
            api_key,
            base_url,
            max_steps,
            model_timeout_sec,
            shell_timeout_sec,
            wall_time_budget_sec,
            working_directory,
            allow_network,
            policy_profile,
            execution_contract_sha256: bridge_hash,
        } => {
            let sidecar_hash = execution_contract_sha256();
            if bridge_hash != sidecar_hash {
                return Err(HeadlessError::ContractMismatch {
                    bridge: bridge_hash,
                    sidecar: sidecar_hash,
                });
            }
            Ok(StartConfig {
                instruction,
                model,
                api_key,
                base_url,
                max_steps,
                model_timeout_sec,
                shell_timeout_sec,
                wall_time_budget_sec,
                working_directory,
                allow_network,
                policy_profile,
            })
        }
        InputMessage::ToolResult { .. } => Err(HeadlessError::ExpectedStart),
    }
}

pub(crate) async fn read_tool_result<R>(
    input: &mut R,
    expected_id: &str,
) -> Result<(Option<i32>, String, String, Option<String>, Option<String>), HeadlessError>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_protocol_line(input)
        .await?
        .ok_or_else(|| HeadlessError::MissingToolResult(expected_id.to_owned()))?;
    match serde_json::from_str::<InputMessage>(&line)? {
        InputMessage::ToolResult {
            id,
            return_code,
            stdout,
            stderr,
            error,
            next_working_directory,
        } if id == expected_id => Ok((return_code, stdout, stderr, error, next_working_directory)),
        InputMessage::ToolResult { id, .. } => Err(HeadlessError::UnexpectedToolResult {
            expected: expected_id.to_owned(),
            actual: id,
        }),
        InputMessage::Start { .. } => Err(HeadlessError::UnexpectedToolResult {
            expected: expected_id.to_owned(),
            actual: "start".to_owned(),
        }),
    }
}

pub(crate) async fn read_protocol_line<R>(input: &mut R) -> Result<Option<String>, HeadlessError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    let bytes = input.read_line(&mut line).await?;
    if bytes == 0 {
        Ok(None)
    } else {
        Ok(Some(line.trim_end().to_owned()))
    }
}

pub(crate) async fn write_output<W>(output: &mut W, message: &OutputMessage) -> Result<(), HeadlessError>
where
    W: AsyncWrite + Unpin,
{
    let mut serialized = serde_json::to_vec(message)?;
    serialized.push(b'\n');
    output.write_all(&serialized).await?;
    output.flush().await?;
    Ok(())
}
