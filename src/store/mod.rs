//! Metadata storage with SQLite - Chronicle v2
//!
//! Key changes from v1:
//! - Added project management (create, link, lookup)
//! - Updated sessions with project linking and assignment
//! - Updated messages with provider_id and content_ref
//! - Removed artifact storage (Antigravity-specific)

mod schema;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

use crate::probe::{MessageMetadata, SessionMetadata, SessionRef, SourceType};

pub use schema::SCHEMA;

pub struct MetadataStore {
    conn: Connection,
}

impl MetadataStore {
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    // ============================================
    // PROVIDERS & SOURCES
    // ============================================

    pub fn ensure_provider(&self, id: &str, name: &str, description: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO providers (id, name, description) VALUES (?, ?, ?)",
            params![id, name, description],
        )?;
        Ok(())
    }

    pub fn ensure_probe_source(
        &self,
        id: &str,
        provider_id: Option<&str>,
        source_name: &str,
        source_type: SourceType,
        base_path: Option<&str>,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO probe_sources (id, provider_id, source_name, source_type, base_path, status) 
             VALUES (?, ?, ?, ?, ?, ?)",
            params![id, provider_id, source_name, source_type.as_str(), base_path, status],
        )?;
        Ok(())
    }

    pub fn update_probe_indexed(&self, probe_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE probe_sources SET last_indexed = datetime('now') WHERE id = ?",
            params![probe_id],
        )?;
        Ok(())
    }

    // ============================================
    // PROJECTS
    // ============================================

    /// Create a new project
    pub fn create_project(
        &self,
        id: &str,
        name: &str,
        project_type: &str,
        primary_path: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects (id, name, type, primary_path, metadata, created_at, last_activity)
             VALUES (?, ?, ?, ?, ?, datetime('now'), datetime('now'))",
            params![id, name, project_type, primary_path, metadata],
        )?;

        // Add primary path to project_paths if provided
        if let Some(path) = primary_path {
            self.add_project_path(id, path, true)?;
        }

        Ok(())
    }

    /// Add a path to a project
    pub fn add_project_path(&self, project_id: &str, path: &str, is_primary: bool) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO project_paths (project_id, path, is_primary, added_at)
             VALUES (?, ?, ?, datetime('now'))",
            params![project_id, path, is_primary],
        )?;
        Ok(())
    }

    /// Add an identifier (git remote, etc.) to a project
    pub fn add_project_identifier(
        &self,
        project_id: &str,
        identifier_type: &str,
        identifier_value: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO project_identifiers (project_id, identifier_type, identifier_value)
             VALUES (?, ?, ?)",
            params![project_id, identifier_type, identifier_value],
        )?;
        Ok(())
    }

    /// Find project by path
    pub fn find_project_by_path(&self, path: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT project_id FROM project_paths WHERE path = ?",
            params![path],
            |row| row.get(0),
        );

        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Find project by git remote
    pub fn find_project_by_git_remote(&self, remote: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT project_id FROM project_identifiers 
             WHERE identifier_type = 'git_remote' AND identifier_value = ?",
            params![remote],
            |row| row.get(0),
        );

        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update project last_activity timestamp
    pub fn touch_project(&self, project_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET last_activity = datetime('now') WHERE id = ?",
            params![project_id],
        )?;
        Ok(())
    }

    // ============================================
    // SESSIONS
    // ============================================

    /// Compute the short_hash for a session, handling duplicates with -N suffix
    fn compute_short_hash(&self, external_id: &str) -> Result<String> {
        // Extract base hash: strip common prefixes, take first 8 chars
        let base = external_id
            .strip_prefix("agent-")
            .or_else(|| external_id.strip_prefix("ses_"))
            .unwrap_or(external_id);
        let base_hash = if base.len() >= 8 { &base[..8] } else { base };

        // Check for existing sessions with same base hash
        let existing_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE short_hash = ?1 OR short_hash LIKE ?2",
            params![base_hash, format!("{}-_", base_hash)],
            |row| row.get(0),
        )?;

        if existing_count == 0 {
            Ok(base_hash.to_string())
        } else {
            // Find the next available suffix
            let max_suffix: Option<i64> = self
                .conn
                .query_row(
                    r#"SELECT MAX(CAST(SUBSTR(short_hash, LENGTH(?1) + 2) AS INTEGER))
                   FROM sessions 
                   WHERE short_hash LIKE ?2"#,
                    params![base_hash, format!("{}-%", base_hash)],
                    |row| row.get(0),
                )
                .ok()
                .flatten();

            let next_suffix = max_suffix.unwrap_or(0) + 1;

            // If this is the first duplicate, rename the original
            if existing_count == 1 {
                let original_has_suffix: bool = self
                    .conn
                    .query_row(
                        "SELECT short_hash LIKE '%-%' FROM sessions WHERE short_hash = ?",
                        params![base_hash],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);

                if !original_has_suffix {
                    self.conn.execute(
                        "UPDATE sessions SET short_hash = ?1 WHERE short_hash = ?2",
                        params![format!("{}-1", base_hash), base_hash],
                    )?;
                }
            }

            Ok(format!("{}-{}", base_hash, next_suffix + 1))
        }
    }

    /// Upsert a session with project linking support
    pub fn upsert_session(
        &self,
        probe_source_id: &str,
        session: &SessionRef,
        metadata: &SessionMetadata,
    ) -> Result<String> {
        let session_id = format!("{}:{}", probe_source_id, session.id);

        // Check if session already exists
        let existing_short_hash: Option<String> = self
            .conn
            .query_row(
                "SELECT short_hash FROM sessions WHERE id = ?",
                params![session_id],
                |row| row.get(0),
            )
            .ok();

        let short_hash = if let Some(hash) = existing_short_hash {
            hash
        } else {
            self.compute_short_hash(&metadata.external_id)?
        };

        // Try to auto-link to a project
        let project_id = self.auto_link_project(metadata)?;
        let project_assignment = if project_id.is_some() {
            "auto"
        } else {
            "auto" // Still 'auto' - means "pending auto-match"
        };

        self.conn.execute(
            r#"INSERT INTO sessions 
               (id, probe_source_id, project_id, project_assignment, external_id, short_hash, 
                title, primary_provider, primary_model, message_count, first_timestamp, 
                last_timestamp, source_path, raw_project_path, raw_git_remote, indexed_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
               ON CONFLICT(id) DO UPDATE SET
                   title = excluded.title,
                   primary_provider = excluded.primary_provider,
                   primary_model = excluded.primary_model,
                   message_count = excluded.message_count,
                   last_timestamp = excluded.last_timestamp,
                   indexed_at = datetime('now')"#,
            params![
                session_id,
                probe_source_id,
                project_id,
                project_assignment,
                metadata.external_id,
                short_hash,
                metadata.title,
                metadata.primary_provider,
                metadata.primary_model,
                metadata.messages.len() as i64,
                metadata.first_timestamp.map(|t| t.to_rfc3339()),
                metadata.last_timestamp.map(|t| t.to_rfc3339()),
                session.source_path.to_string_lossy().to_string(),
                metadata.project_path,
                metadata.git_remote,
            ],
        )?;

        // Update project activity if linked
        if let Some(ref pid) = project_id {
            self.touch_project(pid)?;
        }

        Ok(session_id)
    }

    /// Try to auto-link a session to an existing project
    fn auto_link_project(&self, metadata: &SessionMetadata) -> Result<Option<String>> {
        // Try path matching first
        if let Some(ref path) = metadata.project_path {
            if let Some(project_id) = self.find_project_by_path(path)? {
                return Ok(Some(project_id));
            }
        }

        // Try git remote matching
        if let Some(ref remote) = metadata.git_remote {
            if let Some(project_id) = self.find_project_by_git_remote(remote)? {
                return Ok(Some(project_id));
            }
        }

        Ok(None)
    }

    /// Assign a session to a project (user action)
    pub fn assign_session_to_project(
        &self,
        session_id: &str,
        project_id: Option<&str>,
    ) -> Result<()> {
        let assignment = if project_id.is_some() {
            "user"
        } else {
            "unassigned"
        };

        self.conn.execute(
            "UPDATE sessions SET project_id = ?, project_assignment = ? WHERE id = ?",
            params![project_id, assignment, session_id],
        )?;
        Ok(())
    }

    /// Mark a session as explicitly unassigned
    pub fn unassign_session(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET project_id = NULL, project_assignment = 'unassigned' WHERE id = ?",
            params![session_id],
        )?;
        Ok(())
    }

    // ============================================
    // MESSAGES
    // ============================================

    pub fn insert_messages(&self, session_id: &str, messages: &[MessageMetadata]) -> Result<()> {
        // Delete existing messages for this session
        self.conn.execute(
            "DELETE FROM messages WHERE session_id = ?",
            params![session_id],
        )?;

        for msg in messages {
            // Determine content_ref string (path for JSON files, empty for JSONL)
            let content_ref = msg
                .content_ref
                .content_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());

            let msg_id: i64 = self.conn.query_row(
                r#"INSERT INTO messages 
                   (session_id, uuid, role, provider_id, model, timestamp, source_path, 
                    byte_offset, line_number, content_ref, has_tool_use, has_thinking)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                   RETURNING id"#,
                params![
                    session_id,
                    msg.uuid,
                    msg.role,
                    msg.provider_id,
                    msg.model,
                    msg.timestamp.map(|t| t.to_rfc3339()),
                    msg.content_ref.source_path.to_string_lossy().to_string(),
                    msg.content_ref.byte_offset.map(|o| o as i64),
                    msg.content_ref.line_number.map(|n| n as i64),
                    content_ref,
                    msg.has_tool_use,
                    msg.has_thinking,
                ],
                |row| row.get(0),
            )?;

            // Insert tool uses
            for tool in &msg.tool_uses {
                self.conn.execute(
                    "INSERT INTO tool_uses (message_id, tool_id, tool_name, has_result)
                     VALUES (?, ?, ?, ?)",
                    params![msg_id, tool.tool_id, tool.tool_name, tool.has_result],
                )?;
            }

            // Insert token usage
            if let Some(usage) = &msg.token_usage {
                self.conn.execute(
                    "INSERT OR REPLACE INTO token_usage 
                     (message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens)
                     VALUES (?, ?, ?, ?, ?)",
                    params![
                        msg_id,
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens,
                        usage.cache_creation_tokens,
                    ],
                )?;
            }
        }

        Ok(())
    }

    // ============================================
    // QUERIES
    // ============================================

    pub fn list_sessions(
        &self,
        provider: Option<&str>,
        source: Option<&str>,
    ) -> Result<Vec<SessionRow>> {
        let base_query = r#"SELECT s.id, s.probe_source_id, s.external_id, s.short_hash,
                      s.project_id, s.project_assignment, s.title, s.primary_provider,
                      s.primary_model, s.message_count, s.first_timestamp, 
                      s.last_timestamp, s.raw_project_path, ps.source_name,
                      COALESCE(p.name, ps.provider_id, 'multi') as provider_name,
                      proj.name as project_name
               FROM sessions s
               JOIN probe_sources ps ON s.probe_source_id = ps.id
               LEFT JOIN providers p ON ps.provider_id = p.id
               LEFT JOIN projects proj ON s.project_id = proj.id"#;

        let query = match (provider, source) {
            (Some(_), Some(_)) => format!(
                "{} WHERE (p.id = ?1 OR ps.provider_id = ?1) AND ps.source_name = ?2 ORDER BY s.last_timestamp DESC",
                base_query
            ),
            (Some(_), None) => format!(
                "{} WHERE p.id = ?1 OR ps.provider_id = ?1 ORDER BY s.last_timestamp DESC",
                base_query
            ),
            (None, Some(_)) => format!(
                "{} WHERE ps.source_name = ?1 ORDER BY s.last_timestamp DESC",
                base_query
            ),
            (None, None) => format!("{} ORDER BY s.last_timestamp DESC", base_query),
        };

        let mut stmt = self.conn.prepare(&query)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<SessionRow> {
            Ok(SessionRow {
                id: row.get(0)?,
                probe_source_id: row.get(1)?,
                external_id: row.get(2)?,
                short_hash: row.get(3)?,
                project_id: row.get(4)?,
                project_assignment: row.get(5)?,
                title: row.get(6)?,
                primary_provider: row.get(7)?,
                primary_model: row.get(8)?,
                message_count: row.get(9)?,
                first_timestamp: row.get(10)?,
                last_timestamp: row.get(11)?,
                project_path: row.get(12)?,
                source_name: row.get(13)?,
                provider_name: row.get(14)?,
                project_name: row.get(15)?,
            })
        };

        let rows: Vec<SessionRow> = match (provider, source) {
            (Some(p), Some(s)) => stmt
                .query_map(params![p, s], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
            (Some(p), None) => stmt
                .query_map(params![p], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
            (None, Some(s)) => stmt
                .query_map(params![s], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
            (None, None) => stmt
                .query_map([], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };

        Ok(rows)
    }

    /// Get session by short_hash (primary search) or fallback to id/external_id
    pub fn get_session(&self, query: &str) -> Result<Option<SessionRow>> {
        let row = self.conn.query_row(
            r#"SELECT s.id, s.probe_source_id, s.external_id, s.short_hash,
                      s.project_id, s.project_assignment, s.title, s.primary_provider,
                      s.primary_model, s.message_count, s.first_timestamp, 
                      s.last_timestamp, s.raw_project_path, ps.source_name,
                      COALESCE(p.name, ps.provider_id, 'multi') as provider_name,
                      proj.name as project_name
               FROM sessions s
               JOIN probe_sources ps ON s.probe_source_id = ps.id
               LEFT JOIN providers p ON ps.provider_id = p.id
               LEFT JOIN projects proj ON s.project_id = proj.id
               WHERE s.short_hash = ?1 OR s.short_hash LIKE ?2
                  OR s.id LIKE ?2 OR s.external_id LIKE ?2
               ORDER BY 
                   CASE WHEN s.short_hash = ?1 THEN 0 ELSE 1 END
               LIMIT 1"#,
            params![query, format!("{}%", query)],
            |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    probe_source_id: row.get(1)?,
                    external_id: row.get(2)?,
                    short_hash: row.get(3)?,
                    project_id: row.get(4)?,
                    project_assignment: row.get(5)?,
                    title: row.get(6)?,
                    primary_provider: row.get(7)?,
                    primary_model: row.get(8)?,
                    message_count: row.get(9)?,
                    first_timestamp: row.get(10)?,
                    last_timestamp: row.get(11)?,
                    project_path: row.get(12)?,
                    source_name: row.get(13)?,
                    provider_name: row.get(14)?,
                    project_name: row.get(15)?,
                })
            },
        );

        match row {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, uuid, role, provider_id, model, timestamp, source_path, 
                      byte_offset, line_number, content_ref, has_tool_use, has_thinking
               FROM messages
               WHERE session_id = ?
               ORDER BY COALESCE(line_number, id)"#,
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                uuid: row.get(1)?,
                role: row.get(2)?,
                provider_id: row.get(3)?,
                model: row.get(4)?,
                timestamp: row.get(5)?,
                source_path: row.get(6)?,
                byte_offset: row.get(7)?,
                line_number: row.get(8)?,
                content_ref: row.get(9)?,
                has_tool_use: row.get(10)?,
                has_thinking: row.get(11)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT p.id, p.name, p.type, p.primary_path, p.metadata, 
                      p.created_at, p.last_activity,
                      (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id) as session_count
               FROM projects p
               ORDER BY p.last_activity DESC"#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_type: row.get(2)?,
                primary_path: row.get(3)?,
                metadata: row.get(4)?,
                created_at: row.get(5)?,
                last_activity: row.get(6)?,
                session_count: row.get(7)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

// ============================================
// ROW TYPES
// ============================================

#[derive(Debug)]
pub struct SessionRow {
    pub id: String,
    pub probe_source_id: String,
    pub external_id: String,
    pub short_hash: String,
    pub project_id: Option<String>,
    pub project_assignment: String,
    pub title: Option<String>,
    pub primary_provider: Option<String>,
    pub primary_model: Option<String>,
    pub message_count: i64,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub project_path: Option<String>,
    pub source_name: String,
    pub provider_name: String,
    pub project_name: Option<String>,
}

#[derive(Debug)]
pub struct MessageRow {
    pub id: i64,
    pub uuid: Option<String>,
    pub role: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub timestamp: Option<String>,
    pub source_path: String,
    pub byte_offset: Option<i64>,
    pub line_number: Option<i64>,
    pub content_ref: Option<String>,
    pub has_tool_use: bool,
    pub has_thinking: bool,
}

#[derive(Debug)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub project_type: String,
    pub primary_path: Option<String>,
    pub metadata: Option<String>,
    pub created_at: Option<String>,
    pub last_activity: Option<String>,
    pub session_count: i64,
}
