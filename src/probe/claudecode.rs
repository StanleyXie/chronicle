//! Claude Code probe implementation
//!
//! Extracts conversation history from Claude Code CLI sessions.
//! Data format: JSONL files in ~/.claude/projects/<project_hash>/<session_id>.jsonl

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

use super::{
    ContentRef, IngestionProbe, MessageMetadata, SessionMetadata, SessionRef, SourceType,
    TokenUsage, ToolUseMetadata,
};

pub struct ClaudeCodeProbe {
    base_path: PathBuf,
}

impl ClaudeCodeProbe {
    pub fn new(custom_path: Option<PathBuf>) -> Self {
        let base_path = custom_path.unwrap_or_else(|| {
            let home = dirs::home_dir().unwrap_or_default();
            home.join(".claude/projects")
        });
        Self { base_path }
    }

    /// Extract git remote from project directory if available
    fn extract_git_remote(project_path: &str) -> Option<String> {
        let path = PathBuf::from(project_path);
        let git_config = path.join(".git/config");
        if git_config.exists() {
            if let Ok(content) = std::fs::read_to_string(&git_config) {
                // Simple parsing: find [remote "origin"] section and url line
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

impl IngestionProbe for ClaudeCodeProbe {
    fn id(&self) -> &str {
        "claude:ClaudeCode"
    }

    fn provider(&self) -> &str {
        "claude"
    }

    fn source(&self) -> &str {
        "ClaudeCode"
    }

    fn source_type(&self) -> SourceType {
        SourceType::Single
    }

    fn description(&self) -> &str {
        "Claude Code CLI (Anthropic)"
    }

    fn is_available(&self) -> bool {
        self.base_path.exists()
    }

    fn discover(&self) -> Result<Vec<SessionRef>> {
        let mut sessions = vec![];

        if !self.base_path.exists() {
            return Ok(sessions);
        }

        for project_entry in std::fs::read_dir(&self.base_path)? {
            let project_dir = project_entry?.path();
            if !project_dir.is_dir() {
                continue;
            }

            for file_entry in std::fs::read_dir(&project_dir)? {
                let file_path = file_entry?.path();
                if file_path.extension().map(|e| e == "jsonl").unwrap_or(false) {
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
        let file =
            File::open(&session.source_path).context("Failed to open session file")?;
        let reader = BufReader::new(file);

        let mut messages = vec![];
        let mut first_ts: Option<DateTime<Utc>> = None;
        let mut last_ts: Option<DateTime<Utc>> = None;
        let mut project_path: Option<String> = None;
        let mut title: Option<String> = None;

        // Track provider/model usage for determining primary
        let mut provider_counts: HashMap<String, usize> = HashMap::new();
        let mut model_counts: HashMap<String, usize> = HashMap::new();

        let mut byte_offset: u64 = 0;
        let mut line_number: u32 = 0;

        for line in reader.lines() {
            let line = line?;
            line_number += 1;

            if line.trim().is_empty() {
                byte_offset += line.len() as u64 + 1;
                continue;
            }

            let current_offset = byte_offset;
            byte_offset += line.len() as u64 + 1;

            let json: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Skip queue operations
            if json.get("type").and_then(|v| v.as_str()) == Some("queue-operation") {
                continue;
            }

            // Extract project path from cwd
            if project_path.is_none() {
                project_path = json.get("cwd").and_then(|v| v.as_str()).map(String::from);
            }

            // Parse timestamp
            let timestamp = json
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            if let Some(ts) = timestamp {
                if first_ts.is_none() {
                    first_ts = Some(ts);
                }
                last_ts = Some(ts);
            }

            // Extract role
            let role = json
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(|v| v.as_str())
                .or_else(|| json.get("type").and_then(|v| v.as_str()))
                .unwrap_or("unknown")
                .to_string();

            // Extract model (Claude Code is always Anthropic)
            let msg_model = json
                .get("message")
                .and_then(|m| m.get("model"))
                .and_then(|v| v.as_str())
                .map(String::from);

            if let Some(ref model) = msg_model {
                *model_counts.entry(model.clone()).or_insert(0) += 1;
                *provider_counts.entry("anthropic".to_string()).or_insert(0) += 1;
            }

            // Extract title from first user message
            if title.is_none() && role == "user" {
                let content = json.get("message").and_then(|m| m.get("content"));
                if let Some(c) = content {
                    if let Some(text) = c.as_str() {
                        title = Some(truncate_title(text));
                    } else if let Some(arr) = c.as_array() {
                        for item in arr {
                            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                    title = Some(truncate_title(text));
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // Check for tool use
            let content = json.get("message").and_then(|m| m.get("content"));
            let has_tool_use = content
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                })
                .unwrap_or(false);

            // Extract tool uses
            let tool_uses = content
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            if item.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                Some(ToolUseMetadata {
                                    tool_id: item.get("id").and_then(|v| v.as_str()).map(String::from),
                                    tool_name: item
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                        .to_string(),
                                    has_result: false,
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Check for thinking
            let has_thinking = content
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("thinking"))
                })
                .unwrap_or(false);

            // Extract token usage
            let token_usage = json
                .get("message")
                .and_then(|m| m.get("usage"))
                .map(|usage| TokenUsage {
                    input_tokens: usage.get("input_tokens").and_then(|v| v.as_i64()),
                    output_tokens: usage.get("output_tokens").and_then(|v| v.as_i64()),
                    cache_read_tokens: usage.get("cache_read_input_tokens").and_then(|v| v.as_i64()),
                    cache_creation_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_i64()),
                });

            messages.push(MessageMetadata {
                uuid: json.get("uuid").and_then(|v| v.as_str()).map(String::from),
                role,
                provider_id: Some("anthropic".to_string()),
                model: msg_model,
                timestamp,
                content_ref: ContentRef::jsonl(
                    session.source_path.clone(),
                    current_offset,
                    line_number,
                ),
                has_tool_use,
                has_thinking,
                tool_uses,
                token_usage,
            });
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

        // Extract git remote if we have a project path
        let git_remote = project_path
            .as_ref()
            .and_then(|p| Self::extract_git_remote(p));

        Ok(SessionMetadata {
            external_id: session.id.clone(),
            title,
            project_path,
            git_remote,
            primary_provider,
            primary_model,
            first_timestamp: first_ts,
            last_timestamp: last_ts,
            messages,
        })
    }

    fn get_content(&self, reference: &ContentRef) -> Result<String> {
        let byte_offset = reference.byte_offset.unwrap_or(0);
        let mut file = File::open(&reference.source_path)?;
        file.seek(SeekFrom::Start(byte_offset))?;

        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        Ok(line)
    }
}

/// Truncate a string to make a reasonable title (first 100 chars, first line)
fn truncate_title(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.len() > 100 {
        format!("{}...", &first_line[..97])
    } else {
        first_line.to_string()
    }
}
