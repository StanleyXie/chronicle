//! Ingestion probe trait and registry
//!
//! Chronicle v2: Project-centric observability across multiple AI tools
//!
//! Probe Status:
//! - ClaudeCode: Active (single-provider: Anthropic)
//! - OpenCode: Active (multi-provider)
//! - Zed: Active (multi-provider)
//! - Antigravity: FROZEN (blocked by feasibility, may restart later)

mod claudecode;
mod opencode;
mod zed;

// Antigravity is frozen but kept for reference
// mod antigravity;

pub use claudecode::ClaudeCodeProbe;
pub use opencode::OpenCodeProbe;
pub use zed::ZedProbe;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

use crate::Config;

/// Reference to a session's source location
#[derive(Debug, Clone)]
pub struct SessionRef {
    pub id: String,
    pub source_path: PathBuf,
}

/// Reference to content within a source file
#[derive(Debug, Clone)]
pub struct ContentRef {
    pub source_path: PathBuf,
    /// Byte offset for JSONL files (ClaudeCode)
    pub byte_offset: Option<u64>,
    /// Line number for JSONL files
    pub line_number: Option<u32>,
    /// Path to content file for JSON file sources (OpenCode)
    pub content_path: Option<PathBuf>,
}

impl ContentRef {
    /// Create a content reference for JSONL-based sources
    pub fn jsonl(source_path: PathBuf, byte_offset: u64, line_number: u32) -> Self {
        Self {
            source_path,
            byte_offset: Some(byte_offset),
            line_number: Some(line_number),
            content_path: None,
        }
    }

    /// Create a content reference for JSON file-based sources
    pub fn json_file(source_path: PathBuf, content_path: PathBuf) -> Self {
        Self {
            source_path,
            byte_offset: None,
            line_number: None,
            content_path: Some(content_path),
        }
    }
}

/// Extracted session metadata
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub external_id: String,
    pub title: Option<String>,
    pub project_path: Option<String>,
    pub git_remote: Option<String>,
    pub primary_provider: Option<String>,
    pub primary_model: Option<String>,
    pub first_timestamp: Option<DateTime<Utc>>,
    pub last_timestamp: Option<DateTime<Utc>>,
    pub messages: Vec<MessageMetadata>,
}

/// Extracted message metadata
#[derive(Debug, Clone)]
pub struct MessageMetadata {
    pub uuid: Option<String>,
    pub role: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub content_ref: ContentRef,
    pub has_tool_use: bool,
    pub has_thinking: bool,
    pub tool_uses: Vec<ToolUseMetadata>,
    pub token_usage: Option<TokenUsage>,
}

/// Tool use metadata
#[derive(Debug, Clone)]
pub struct ToolUseMetadata {
    pub tool_id: Option<String>,
    pub tool_name: String,
    pub has_result: bool,
}

/// Token usage metadata
#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
}

/// Source type indicator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    /// Single provider (e.g., ClaudeCode -> Anthropic only)
    Single,
    /// Multi-provider (e.g., OpenCode, Zed -> can use any provider)
    Multi,
}

impl SourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceType::Single => "single",
            SourceType::Multi => "multi",
        }
    }
}

/// Ingestion probe trait
pub trait IngestionProbe: Send + Sync {
    /// Unique identifier: "{provider}:{source}" or "{source}:{source}" for multi-provider
    fn id(&self) -> &str;

    /// Provider name (for single-provider) or source name (for multi-provider)
    fn provider(&self) -> &str;

    /// Probe source identifier
    fn source(&self) -> &str;

    /// Whether this is a single or multi-provider source
    fn source_type(&self) -> SourceType;

    /// Human-readable description
    fn description(&self) -> &str;

    /// Check if this probe's data source exists
    fn is_available(&self) -> bool;

    /// Discover sessions to index
    fn discover(&self) -> Result<Vec<SessionRef>>;

    /// Extract metadata from a session
    fn extract_metadata(&self, session: &SessionRef) -> Result<SessionMetadata>;

    /// Get raw content by reference (lazy load)
    fn get_content(&self, reference: &ContentRef) -> Result<String>;
}

/// Registry of available probes
pub struct ProbeRegistry {
    probes: Vec<Box<dyn IngestionProbe>>,
}

impl ProbeRegistry {
    pub fn new(config: &Config) -> Self {
        let mut registry = Self { probes: vec![] };

        // Register Claude Code probe (single-provider: Anthropic)
        if config.is_probe_enabled("claude:ClaudeCode") {
            let claudecode = ClaudeCodeProbe::new(config.probe_path("claude:ClaudeCode"));
            registry.register(Box::new(claudecode));
        }

        // Register OpenCode probe (multi-provider)
        if config.is_probe_enabled("opencode:OpenCode") {
            let opencode = OpenCodeProbe::new(config.probe_path("opencode:OpenCode"));
            registry.register(Box::new(opencode));
        }

        // Register Zed probe (multi-provider)
        if config.is_probe_enabled("zed:Zed") {
            let zed = ZedProbe::new(config.probe_path("zed:Zed"));
            registry.register(Box::new(zed));
        }

        // Antigravity is FROZEN - not registered
        // Reason: Blocked by feasibility, may restart later
        // The probe code is preserved in antigravity.rs for reference

        registry
    }

    pub fn register(&mut self, probe: Box<dyn IngestionProbe>) {
        self.probes.push(probe);
    }

    pub fn available_probes(&self) -> Vec<&dyn IngestionProbe> {
        self.probes
            .iter()
            .filter(|p| p.is_available())
            .map(|p| p.as_ref())
            .collect()
    }

    pub fn all_probes(&self) -> Vec<&dyn IngestionProbe> {
        self.probes.iter().map(|p| p.as_ref()).collect()
    }

    pub fn get_probe(&self, id: &str) -> Option<&dyn IngestionProbe> {
        self.probes
            .iter()
            .find(|p| p.id() == id)
            .map(|p| p.as_ref())
    }
}
