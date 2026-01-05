//! SQLite schema definition - Chronicle v2
//! 
//! Key changes from v1:
//! - Added projects, project_paths, project_identifiers for project-centric view
//! - Added session_duplicates for deduplication tracking
//! - Updated sessions with project linking and assignment tracking
//! - Updated messages with provider_id and content_ref
//! - Updated probe_sources with source_type and status
//! - Removed artifacts table (Antigravity-specific, now frozen)

pub const SCHEMA: &str = r#"
-- ============================================
-- PROVIDERS & SOURCES
-- ============================================

-- Model providers (anthropic, openai, google, etc.)
CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT
);

-- Probe sources (tools that capture AI conversations)
CREATE TABLE IF NOT EXISTS probe_sources (
    id TEXT PRIMARY KEY,                   -- 'opencode:OpenCode', 'zed:Zed', etc.
    provider_id TEXT,                      -- NULL for multi-provider sources
    source_name TEXT NOT NULL,             -- 'OpenCode', 'Zed', 'ClaudeCode'
    source_type TEXT DEFAULT 'single',     -- 'single' | 'multi' provider
    base_path TEXT,
    status TEXT DEFAULT 'active',          -- 'active', 'frozen', 'deprecated'
    last_indexed DATETIME,
    FOREIGN KEY(provider_id) REFERENCES providers(id)
);

-- ============================================
-- PROJECTS (New in v2)
-- ============================================

-- Projects aggregate sessions across different AI tools
-- Types: 'code' (git-based), 'research' (folder-based), 'general' (catch-all)
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,                   -- UUID
    name TEXT NOT NULL,                    -- User-friendly name
    type TEXT DEFAULT 'code',              -- 'code', 'research', 'general'
    primary_path TEXT,                     -- Main directory (nullable for virtual projects)
    metadata TEXT,                         -- JSON: type-specific fields
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_activity DATETIME
);

-- Multiple paths can map to the same project
-- (symlinks, different mount points, renamed folders)
CREATE TABLE IF NOT EXISTS project_paths (
    id INTEGER PRIMARY KEY,
    project_id TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,             -- Normalized absolute path
    is_primary BOOLEAN DEFAULT FALSE,
    added_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
);

-- Git remotes and other identifiers for cross-machine matching
CREATE TABLE IF NOT EXISTS project_identifiers (
    id INTEGER PRIMARY KEY,
    project_id TEXT NOT NULL,
    identifier_type TEXT NOT NULL,         -- 'git_remote', 'git_worktree', 'custom'
    identifier_value TEXT NOT NULL,
    UNIQUE(identifier_type, identifier_value),
    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
);

-- ============================================
-- SESSIONS
-- ============================================

-- Sessions (conversations) - updated with project linking
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    probe_source_id TEXT NOT NULL,
    project_id TEXT,                       -- NULL = unassigned/pending
    project_assignment TEXT DEFAULT 'auto', -- 'auto', 'user', 'unassigned'
    external_id TEXT,                      -- Original ID from source
    short_hash TEXT NOT NULL,              -- 8-char display hash with optional -N suffix
    title TEXT,                            -- Session title/summary
    primary_provider TEXT,                 -- Most-used provider in session
    primary_model TEXT,                    -- Most-used model in session
    message_count INTEGER DEFAULT 0,
    first_timestamp DATETIME,
    last_timestamp DATETIME,
    source_path TEXT NOT NULL,             -- Path to source file/dir
    raw_project_path TEXT,                 -- Original path from source (for linking)
    raw_git_remote TEXT,                   -- Git remote if available
    indexed_at DATETIME,
    FOREIGN KEY(probe_source_id) REFERENCES probe_sources(id),
    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE SET NULL
);

-- ============================================
-- MESSAGES
-- ============================================

-- Message index (metadata only, no content stored)
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    uuid TEXT,
    role TEXT NOT NULL,                    -- 'user', 'assistant', 'system', 'tool'
    provider_id TEXT,                      -- 'anthropic', 'openai', 'google', etc.
    model TEXT,                            -- 'claude-opus-4-5', 'gpt-4', etc.
    timestamp DATETIME,
    source_path TEXT NOT NULL,
    byte_offset INTEGER,                   -- For JSONL sources (ClaudeCode)
    line_number INTEGER,                   -- For JSONL
    content_ref TEXT,                      -- For JSON file sources (OpenCode part path)
    has_tool_use BOOLEAN DEFAULT FALSE,
    has_thinking BOOLEAN DEFAULT FALSE,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

-- ============================================
-- TOOL USES
-- ============================================

CREATE TABLE IF NOT EXISTS tool_uses (
    id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL,
    tool_id TEXT,
    tool_name TEXT NOT NULL,
    has_result BOOLEAN DEFAULT FALSE,
    FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);

-- ============================================
-- TOKEN USAGE
-- ============================================

CREATE TABLE IF NOT EXISTS token_usage (
    message_id INTEGER PRIMARY KEY,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_creation_tokens INTEGER,
    FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);

-- ============================================
-- DEDUPLICATION (New in v2)
-- ============================================

-- Track potential duplicate sessions across sources
CREATE TABLE IF NOT EXISTS session_duplicates (
    id INTEGER PRIMARY KEY,
    session_a TEXT NOT NULL,
    session_b TEXT NOT NULL,
    confidence REAL,                       -- 0.0 to 1.0
    detection_method TEXT,                 -- 'content_hash', 'timestamp', 'tool_ids'
    detected_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    resolved BOOLEAN DEFAULT FALSE,
    resolution TEXT,                       -- 'merged', 'kept_both', 'false_positive'
    resolved_at DATETIME,
    UNIQUE(session_a, session_b),
    FOREIGN KEY(session_a) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(session_b) REFERENCES sessions(id) ON DELETE CASCADE
);

-- ============================================
-- INDEXES
-- ============================================

-- Sessions indexes
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
CREATE INDEX IF NOT EXISTS idx_sessions_assignment ON sessions(project_assignment);
CREATE INDEX IF NOT EXISTS idx_sessions_timestamp ON sessions(last_timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_probe ON sessions(probe_source_id);
CREATE INDEX IF NOT EXISTS idx_sessions_short_hash ON sessions(short_hash);

-- Messages indexes
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE INDEX IF NOT EXISTS idx_messages_provider ON messages(provider_id);
CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
CREATE INDEX IF NOT EXISTS idx_messages_role ON messages(role);

-- Tool uses indexes
CREATE INDEX IF NOT EXISTS idx_tool_uses_name ON tool_uses(tool_name);
CREATE INDEX IF NOT EXISTS idx_tool_uses_message ON tool_uses(message_id);

-- Project indexes
CREATE INDEX IF NOT EXISTS idx_project_paths_path ON project_paths(path);
CREATE INDEX IF NOT EXISTS idx_project_ids_value ON project_identifiers(identifier_value);
CREATE INDEX IF NOT EXISTS idx_project_ids_type ON project_identifiers(identifier_type);

-- Deduplication indexes
CREATE INDEX IF NOT EXISTS idx_duplicates_unresolved ON session_duplicates(resolved) WHERE resolved = FALSE;
"#;
