//! OpenCode probe implementation
//!
//! Extracts conversation history from OpenCode CLI sessions.
//! Data format: JSON files in ~/.local/share/opencode/storage/
//!   - session/{project_hash}/ses_*.json - Session metadata
//!   - message/{session_id}/msg_*.json - Message metadata  
//!   - part/{message_id}/prt_*.json - Message content parts
//!
//! OpenCode is a multi-provider source (can use Anthropic, OpenAI, Google, etc.)

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use super::{
    ContentRef, IngestionProbe, MessageMetadata, SessionMetadata, SessionRef, SourceType,
    TokenUsage, ToolUseMetadata,
};

pub struct OpenCodeProbe {
    base_path: PathBuf,
}

// OpenCode data structures
#[derive(Debug, Deserialize)]
struct _OpenCodeSession {
    _id: String,
    #[serde(rename = "projectID")]
    _project_id: Option<String>,
    directory: Option<String>,
    title: Option<String>,
    time: Option<SessionTime>,
}

#[derive(Debug, Deserialize)]
struct SessionTime {
    created: Option<i64>,
    updated: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeMessage {
    id: String,
    #[serde(rename = "sessionID")]
    _session_id: String,
    role: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    model: Option<MessageModel>,
    time: Option<MessageTime>,
}

#[derive(Debug, Deserialize)]
struct MessageModel {
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageTime {
    created: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OpenCodePart {
    _id: String,
    #[serde(rename = "sessionID")]
    _session_id: String,
    #[serde(rename = "messageID")]
    _message_id: String,
    #[serde(rename = "type")]
    part_type: String,
    // For text parts
    _text: Option<String>,
    // For tool parts
    tool: Option<String>,
    #[serde(rename = "callID")]
    call_id: Option<String>,
    state: Option<ToolState>,
    // For step-finish parts
    tokens: Option<TokenInfo>,
}

#[derive(Debug, Deserialize)]
struct ToolState {
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    input: Option<i64>,
    output: Option<i64>,
    _reasoning: Option<i64>,
    cache: Option<CacheInfo>,
}

#[derive(Debug, Deserialize)]
struct CacheInfo {
    read: Option<i64>,
    write: Option<i64>,
}

impl OpenCodeProbe {
    pub fn new(custom_path: Option<PathBuf>) -> Self {
        let base_path = custom_path.unwrap_or_else(|| {
            let home = dirs::home_dir().unwrap_or_default();
            home.join(".local/share/opencode/storage")
        });
        Self { base_path }
    }

    fn session_dir(&self) -> PathBuf {
        self.base_path.join("session")
    }

    fn message_dir(&self) -> PathBuf {
        self.base_path.join("message")
    }

    fn part_dir(&self) -> PathBuf {
        self.base_path.join("part")
    }

    /// Convert millisecond timestamp to DateTime
    fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
        Utc.timestamp_millis_opt(ms).single()
    }

    /// Extract git remote from directory if available
    fn extract_git_remote(directory: &str) -> Option<String> {
        let path = PathBuf::from(directory);
        let git_config = path.join(".git/config");
        if git_config.exists() {
            if let Ok(content) = fs::read_to_string(&git_config) {
                let mut in_origin = false;
                for line in content.lines() {
                    if line.contains("[remote \"origin\"]") {
                        in_origin = true;
                    } else if in_origin && line.trim().starts_with("url = ") {
                        return Some(line.trim().strip_prefix("url = ")?.to_string());
                    } else if line.starts_with('[') {
                        in_origin = false;
                    }
                }
            }
        }
        None
    }
}

impl IngestionProbe for OpenCodeProbe {
    fn id(&self) -> &str {
        "opencode:OpenCode"
    }

    fn provider(&self) -> &str {
        "opencode"
    }

    fn source(&self) -> &str {
        "OpenCode"
    }

    fn source_type(&self) -> SourceType {
        SourceType::Multi
    }

    fn description(&self) -> &str {
        "OpenCode CLI (multi-provider)"
    }

    fn is_available(&self) -> bool {
        self.base_path.exists() && self.session_dir().exists()
    }

    fn discover(&self) -> Result<Vec<SessionRef>> {
        let mut sessions = vec![];
        let session_dir = self.session_dir();

        if !session_dir.exists() {
            return Ok(sessions);
        }

        // Iterate through project directories (including "global")
        for project_entry in fs::read_dir(&session_dir)? {
            let project_dir = project_entry?.path();
            if !project_dir.is_dir() {
                continue;
            }

            // Find session files in each project directory
            for file_entry in fs::read_dir(&project_dir)? {
                let file_path = file_entry?.path();
                if file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("ses_") && n.ends_with(".json"))
                    .unwrap_or(false)
                {
                    let session_id = file_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    sessions.push(SessionRef {
                        id: session_id,
                        source_path: file_path,
                    });
                }
            }
        }

        Ok(sessions)
    }

    fn extract_metadata(&self, session: &SessionRef) -> Result<SessionMetadata> {
        // Read session file
        let session_content =
            fs::read_to_string(&session.source_path).context("Failed to read session file")?;
        let session_data: _OpenCodeSession =
            serde_json::from_str(&session_content).context("Failed to parse session JSON")?;

        // Get timestamps from session
        let first_timestamp = session_data
            .time
            .as_ref()
            .and_then(|t| t.created)
            .and_then(Self::ms_to_datetime);
        let last_timestamp = session_data
            .time
            .as_ref()
            .and_then(|t| t.updated)
            .and_then(Self::ms_to_datetime);

        // Get project path (directory field, or resolve from project_id)
        let project_path = session_data.directory.clone();
        let git_remote = project_path
            .as_ref()
            .and_then(|p| Self::extract_git_remote(p));

        // Read messages for this session
        let message_session_dir = self.message_dir().join(&session.id);
        let mut messages = vec![];
        let mut provider_counts: HashMap<String, usize> = HashMap::new();
        let mut model_counts: HashMap<String, usize> = HashMap::new();

        if message_session_dir.exists() {
            let mut msg_files: Vec<_> = fs::read_dir(&message_session_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("msg_") && n.ends_with(".json"))
                        .unwrap_or(false)
                })
                .collect();

            // Sort by filename (which contains timestamp-based ID)
            msg_files.sort_by_key(|e| e.path());

            for msg_entry in msg_files {
                let msg_path = msg_entry.path();
                let msg_content = fs::read_to_string(&msg_path)?;
                let msg_data: OpenCodeMessage = match serde_json::from_str(&msg_content) {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                // Extract provider and model
                let provider_id = msg_data
                    .provider_id
                    .or_else(|| msg_data.model.as_ref().and_then(|m| m.provider_id.clone()));
                let model_id = msg_data
                    .model_id
                    .or_else(|| msg_data.model.as_ref().and_then(|m| m.model_id.clone()));

                if let Some(ref provider) = provider_id {
                    *provider_counts.entry(provider.clone()).or_insert(0) += 1;
                }
                if let Some(ref model) = model_id {
                    *model_counts.entry(model.clone()).or_insert(0) += 1;
                }

                // Get message timestamp
                let timestamp = msg_data
                    .time
                    .as_ref()
                    .and_then(|t| t.created)
                    .and_then(Self::ms_to_datetime);

                // Determine role (default to "assistant" for model responses)
                let role = msg_data.role.unwrap_or_else(|| {
                    if provider_id.is_some() {
                        "assistant".to_string()
                    } else {
                        "user".to_string()
                    }
                });

                // Read parts for this message to get tool usage and tokens
                let part_msg_dir = self.part_dir().join(&msg_data.id);
                let mut has_tool_use = false;
                let mut has_thinking = false;
                let mut tool_uses = vec![];
                let mut token_usage: Option<TokenUsage> = None;
                let mut first_text_part_path: Option<PathBuf> = None;

                if part_msg_dir.exists() {
                    for part_entry in fs::read_dir(&part_msg_dir)? {
                        let part_path = part_entry?.path();
                        if !part_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("prt_") && n.ends_with(".json"))
                            .unwrap_or(false)
                        {
                            continue;
                        }

                        let part_content = fs::read_to_string(&part_path)?;
                        let part_data: OpenCodePart = match serde_json::from_str(&part_content) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        match part_data.part_type.as_str() {
                            "text" => {
                                if first_text_part_path.is_none() {
                                    first_text_part_path = Some(part_path.clone());
                                }
                            }
                            "tool" => {
                                has_tool_use = true;
                                tool_uses.push(ToolUseMetadata {
                                    tool_id: part_data.call_id,
                                    tool_name: part_data
                                        .tool
                                        .unwrap_or_else(|| "unknown".to_string()),
                                    has_result: part_data
                                        .state
                                        .as_ref()
                                        .map(|s| s.status.as_deref() == Some("completed"))
                                        .unwrap_or(false),
                                });
                            }
                            "step-finish" => {
                                if let Some(tokens) = part_data.tokens {
                                    token_usage = Some(TokenUsage {
                                        input_tokens: tokens.input,
                                        output_tokens: tokens.output,
                                        cache_read_tokens: tokens
                                            .cache
                                            .as_ref()
                                            .and_then(|c| c.read),
                                        cache_creation_tokens: tokens
                                            .cache
                                            .as_ref()
                                            .and_then(|c| c.write),
                                    });
                                }
                            }
                            "thinking" => {
                                has_thinking = true;
                            }
                            _ => {}
                        }
                    }
                }

                // Create content reference pointing to first text part or message file
                let content_ref = if let Some(text_path) = first_text_part_path {
                    ContentRef::json_file(msg_path.clone(), text_path)
                } else {
                    ContentRef::json_file(msg_path.clone(), msg_path.clone())
                };

                messages.push(MessageMetadata {
                    uuid: Some(msg_data.id),
                    role,
                    provider_id,
                    model: model_id,
                    timestamp,
                    content_ref,
                    has_tool_use,
                    has_thinking,
                    tool_uses,
                    token_usage,
                });
            }
        }

        // Determine primary provider/model
        let primary_provider = provider_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(provider, _)| provider);

        let primary_model = model_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(model, _)| model);

        Ok(SessionMetadata {
            external_id: session.id.clone(),
            title: session_data.title,
            project_path,
            git_remote,
            primary_provider,
            primary_model,
            first_timestamp,
            last_timestamp,
            messages,
        })
    }

    fn get_content(&self, reference: &ContentRef) -> Result<String> {
        // For OpenCode, content is in separate part files
        if let Some(content_path) = &reference.content_path {
            let content = fs::read_to_string(content_path)?;

            // Try to extract text from the JSON
            if let Ok(json) = serde_json::from_str::<Value>(&content) {
                // For text parts, extract the text field
                if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
                    return Ok(text.to_string());
                }
                // For tool parts, return the state output
                if let Some(state) = json.get("state") {
                    if let Some(output) = state.get("output").and_then(|o| o.as_str()) {
                        return Ok(output.to_string());
                    }
                }
            }

            // Return raw content if parsing fails
            return Ok(content);
        }

        // Fallback to source_path
        fs::read_to_string(&reference.source_path).context("Failed to read content")
    }
}
