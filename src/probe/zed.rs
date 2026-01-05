//! Zed Editor probe implementation
//!
//! Extracts conversation history from Zed's AI assistant threads.
//! Data format: SQLite database at ~/Library/Application Support/Zed/threads/threads.db
//!   - threads table with zstd-compressed JSON in data column
//!
//! Zed is a multi-provider source (can use Anthropic, OpenAI, Google via Copilot, etc.)

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use super::{
    ContentRef, IngestionProbe, MessageMetadata, SessionMetadata, SessionRef, SourceType,
    TokenUsage, ToolUseMetadata,
};

pub struct ZedProbe {
    db_path: PathBuf,
}

// Zed data structures (from decompressed JSON)
#[derive(Debug, Deserialize)]
struct ZedThread {
    title: Option<String>,
    messages: Vec<ZedMessage>,
    updated_at: Option<String>,
    model: Option<ZedModel>,
    initial_project_snapshot: Option<ProjectSnapshot>,
    cumulative_token_usage: Option<HashMap<String, i64>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ZedMessage {
    User(UserMessage),
    Agent(AgentMessage),
    Resume, // String "Resume" marker
}

#[derive(Debug, Deserialize)]
struct UserMessage {
    #[serde(rename = "User")]
    user: UserContent,
}

#[derive(Debug, Deserialize)]
struct UserContent {
    id: Option<String>,
    content: Vec<ContentItem>,
}

#[derive(Debug, Deserialize)]
struct AgentMessage {
    #[serde(rename = "Agent")]
    agent: AgentContent,
}

#[derive(Debug, Deserialize)]
struct AgentContent {
    content: Vec<ContentItem>,
    tool_results: Option<HashMap<String, ToolResult>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ContentItem {
    Text { #[serde(rename = "Text")] text: String },
    ToolUse {
        #[serde(rename = "ToolUse")]
        tool_use: ToolUseInfo,
    },
    Other(Value),
}

#[derive(Debug, Deserialize)]
struct ToolUseInfo {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolResult {
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    is_error: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ZedModel {
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectSnapshot {
    worktree_snapshots: Option<Vec<WorktreeSnapshot>>,
}

#[derive(Debug, Deserialize)]
struct WorktreeSnapshot {
    worktree_path: Option<String>,
    git_state: Option<GitState>,
}

#[derive(Debug, Deserialize)]
struct GitState {
    remote_url: Option<String>,
}

impl ZedProbe {
    pub fn new(custom_path: Option<PathBuf>) -> Self {
        let db_path = custom_path.unwrap_or_else(|| {
            let home = dirs::home_dir().unwrap_or_default();
            home.join("Library/Application Support/Zed/threads/threads.db")
        });
        Self { db_path }
    }

    /// Decompress zstd-compressed data
    fn decompress_zstd(data: &[u8]) -> Result<String> {
        let mut decoder = zstd::Decoder::new(data)?;
        let mut decompressed = String::new();
        decoder.read_to_string(&mut decompressed)?;
        Ok(decompressed)
    }

    /// Open database in read-only mode
    fn open_db(&self) -> Result<Connection> {
        Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("Failed to open Zed threads database")
    }
}

impl IngestionProbe for ZedProbe {
    fn id(&self) -> &str {
        "zed:Zed"
    }

    fn provider(&self) -> &str {
        "zed"
    }

    fn source(&self) -> &str {
        "Zed"
    }

    fn source_type(&self) -> SourceType {
        SourceType::Multi
    }

    fn description(&self) -> &str {
        "Zed Editor AI Assistant (multi-provider)"
    }

    fn is_available(&self) -> bool {
        self.db_path.exists()
    }

    fn discover(&self) -> Result<Vec<SessionRef>> {
        let mut sessions = vec![];

        if !self.is_available() {
            return Ok(sessions);
        }

        let conn = self.open_db()?;
        let mut stmt = conn.prepare("SELECT id FROM threads")?;

        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            Ok(id)
        })?;

        for row in rows {
            let id = row?;
            sessions.push(SessionRef {
                id: id.clone(),
                source_path: self.db_path.clone(),
            });
        }

        Ok(sessions)
    }

    fn extract_metadata(&self, session: &SessionRef) -> Result<SessionMetadata> {
        let conn = self.open_db()?;

        // Query thread data
        let (summary, updated_at, data_type, data): (String, String, String, Vec<u8>) = conn
            .query_row(
                "SELECT summary, updated_at, data_type, data FROM threads WHERE id = ?",
                [&session.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .context("Failed to query thread")?;

        // Decompress data based on data_type
        let json_str = if data_type == "zstd" {
            Self::decompress_zstd(&data)?
        } else {
            String::from_utf8(data).context("Invalid UTF-8 in thread data")?
        };

        // Parse JSON
        let thread: ZedThread =
            serde_json::from_str(&json_str).context("Failed to parse thread JSON")?;

        // Extract timestamps
        let last_timestamp = thread
            .updated_at
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|| {
                DateTime::parse_from_rfc3339(&updated_at)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            });

        // Extract project info from snapshot
        let (project_path, git_remote) = thread
            .initial_project_snapshot
            .as_ref()
            .and_then(|snap| snap.worktree_snapshots.as_ref())
            .and_then(|snapshots| snapshots.first())
            .map(|ws| {
                (
                    ws.worktree_path.clone(),
                    ws.git_state.as_ref().and_then(|g| g.remote_url.clone()),
                )
            })
            .unwrap_or((None, None));

        // Extract provider/model from thread-level model
        let session_provider = thread.model.as_ref().and_then(|m| m.provider.clone());
        let session_model = thread.model.as_ref().and_then(|m| m.model.clone());

        // Process messages
        let mut messages = vec![];
        let mut provider_counts: HashMap<String, usize> = HashMap::new();
        let mut model_counts: HashMap<String, usize> = HashMap::new();
        let mut first_timestamp: Option<DateTime<Utc>> = None;

        // Count session-level provider/model
        if let Some(ref provider) = session_provider {
            *provider_counts.entry(provider.clone()).or_insert(0) += 1;
        }
        if let Some(ref model) = session_model {
            *model_counts.entry(model.clone()).or_insert(0) += 1;
        }

        for (idx, msg) in thread.messages.iter().enumerate() {
            match msg {
                ZedMessage::User(user_msg) => {
                    let has_tool_use = false;
                    let tool_uses = vec![];

                    messages.push(MessageMetadata {
                        uuid: user_msg.user.id.clone(),
                        role: "user".to_string(),
                        provider_id: None,
                        model: None,
                        timestamp: if idx == 0 { first_timestamp } else { None },
                        content_ref: ContentRef {
                            source_path: self.db_path.clone(),
                            byte_offset: None,
                            line_number: Some(idx as u32),
                            content_path: None,
                        },
                        has_tool_use,
                        has_thinking: false,
                        tool_uses,
                        token_usage: None,
                    });

                    // Set first timestamp from first user message
                    if first_timestamp.is_none() {
                        first_timestamp = last_timestamp;
                    }
                }
                ZedMessage::Agent(agent_msg) => {
                    // Check for tool uses in content
                    let mut has_tool_use = false;
                    let mut tool_uses = vec![];

                    for item in &agent_msg.agent.content {
                        if let ContentItem::ToolUse { tool_use } = item {
                            has_tool_use = true;
                            let has_result = agent_msg
                                .agent
                                .tool_results
                                .as_ref()
                                .and_then(|results| {
                                    tool_use.id.as_ref().and_then(|id| results.get(id))
                                })
                                .is_some();

                            tool_uses.push(ToolUseMetadata {
                                tool_id: tool_use.id.clone(),
                                tool_name: tool_use
                                    .name
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string()),
                                has_result,
                            });
                        }
                    }

                    messages.push(MessageMetadata {
                        uuid: None,
                        role: "assistant".to_string(),
                        provider_id: session_provider.clone(),
                        model: session_model.clone(),
                        timestamp: None,
                        content_ref: ContentRef {
                            source_path: self.db_path.clone(),
                            byte_offset: None,
                            line_number: Some(idx as u32),
                            content_path: None,
                        },
                        has_tool_use,
                        has_thinking: false,
                        tool_uses,
                        token_usage: None, // Token usage is at thread level in Zed
                    });
                }
                ZedMessage::Resume => {
                    // Skip resume markers
                }
            }
        }

        // Use title from thread or summary from DB
        let title = thread.title.or(Some(summary)).filter(|t| !t.is_empty());

        // Determine primary provider/model
        let primary_provider = provider_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(provider, _)| provider)
            .or(session_provider);

        let primary_model = model_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(model, _)| model)
            .or(session_model);

        Ok(SessionMetadata {
            external_id: session.id.clone(),
            title,
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
        // For Zed, we need to query the database and extract the specific message
        let conn = self.open_db()?;

        // The source_path contains the thread ID embedded in the session ID
        // We need to get the message by index (line_number)
        let thread_id = reference
            .source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // If we have a thread ID in the path, use it; otherwise extract from context
        // This is a simplified implementation - in practice, we'd need the session ID
        let (data_type, data): (String, Vec<u8>) = conn
            .query_row(
                "SELECT data_type, data FROM threads LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("Failed to query thread for content")?;

        let json_str = if data_type == "zstd" {
            Self::decompress_zstd(&data)?
        } else {
            String::from_utf8(data)?
        };

        let thread: ZedThread = serde_json::from_str(&json_str)?;

        // Get message by index
        if let Some(line_num) = reference.line_number {
            if let Some(msg) = thread.messages.get(line_num as usize) {
                match msg {
                    ZedMessage::User(user_msg) => {
                        let texts: Vec<&str> = user_msg
                            .user
                            .content
                            .iter()
                            .filter_map(|item| {
                                if let ContentItem::Text { text } = item {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        return Ok(texts.join("\n"));
                    }
                    ZedMessage::Agent(agent_msg) => {
                        let texts: Vec<&str> = agent_msg
                            .agent
                            .content
                            .iter()
                            .filter_map(|item| {
                                if let ContentItem::Text { text } = item {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        return Ok(texts.join("\n"));
                    }
                    ZedMessage::Resume => {
                        return Ok("[Resume]".to_string());
                    }
                }
            }
        }

        Ok(String::new())
    }
}
