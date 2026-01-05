//! Antigravity (Gemini) probe implementation

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;

use super::{
    ArtifactMetadata, ContentRef, IngestionProbe, MessageMetadata, SessionMetadata, SessionRef,
};

pub struct AntigravityProbe {
    base_path: PathBuf,
}

impl AntigravityProbe {
    pub fn new(custom_path: Option<PathBuf>) -> Self {
        let base_path = custom_path.unwrap_or_else(|| {
            let home = dirs::home_dir().unwrap_or_default();
            home.join(".gemini/antigravity/brain")
        });
        Self { base_path }
    }
    
    fn infer_artifact_type(filename: &str) -> Option<String> {
        if filename.contains("task") {
            Some("task".to_string())
        } else if filename.contains("implementation_plan") || filename.contains("plan") {
            Some("plan".to_string())
        } else if filename.contains("walkthrough") {
            Some("walkthrough".to_string())
        } else if filename.ends_with(".webp") || filename.ends_with(".png") {
            Some("media".to_string())
        } else {
            None
        }
    }
    
    fn parse_version(filename: &str) -> i32 {
        // Parse .resolved.N format
        if let Some(pos) = filename.rfind(".resolved.") {
            filename[pos + 10..].parse().unwrap_or(0)
        } else if filename.contains(".resolved") {
            0
        } else {
            -1 // Current version (not a snapshot)
        }
    }
}

impl IngestionProbe for AntigravityProbe {
    fn id(&self) -> &str {
        "gemini:Antigravity"
    }
    
    fn provider(&self) -> &str {
        "gemini"
    }
    
    fn source(&self) -> &str {
        "Antigravity"
    }
    
    fn description(&self) -> &str {
        "Antigravity (Gemini) brain artifacts"
    }
    
    fn is_available(&self) -> bool {
        self.base_path.exists()
    }
    
    fn discover(&self) -> Result<Vec<SessionRef>> {
        let mut sessions = vec![];
        
        if !self.base_path.exists() {
            return Ok(sessions);
        }
        
        for entry in fs::read_dir(&self.base_path)? {
            let entry = entry?;
            let path = entry.path();
            
            if !path.is_dir() {
                continue;
            }
            
            let session_id = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            
            // Basic UUID check
            if session_id.len() < 30 {
                continue;
            }
            
            sessions.push(SessionRef {
                id: session_id,
                source_path: path,
            });
        }
        
        Ok(sessions)
    }
    
    fn extract_metadata(&self, session: &SessionRef) -> Result<SessionMetadata> {
        // Antigravity doesn't have message-level metadata in the same way
        // We use artifacts as the primary data
        
        let mut first_ts: Option<DateTime<Utc>> = None;
        let mut last_ts: Option<DateTime<Utc>> = None;
        
        // Scan for modification times
        for entry in walkdir::WalkDir::new(&session.source_path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        let ts: DateTime<Utc> = modified.into();
                        if first_ts.is_none() || ts < first_ts.unwrap() {
                            first_ts = Some(ts);
                        }
                        if last_ts.is_none() || ts > last_ts.unwrap() {
                            last_ts = Some(ts);
                        }
                    }
                }
            }
        }
        
        Ok(SessionMetadata {
            external_id: session.id.clone(),
            project_path: None,
            model: Some("gemini".to_string()),
            first_timestamp: first_ts,
            last_timestamp: last_ts,
            messages: vec![], // Antigravity uses artifacts, not messages
        })
    }
    
    fn extract_artifacts(&self, session: &SessionRef) -> Result<Vec<ArtifactMetadata>> {
        let mut artifacts = vec![];
        
        for entry in walkdir::WalkDir::new(&session.source_path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            
            // Skip hidden files (except .resolved)
            if filename.starts_with('.') && !filename.contains("resolved") {
                continue;
            }
            
            let ext = path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            
            let is_binary = matches!(ext, "webp" | "png" | "jpg" | "jpeg" | "pb");
            
            let last_modified = path.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| t.into());
            
            artifacts.push(ArtifactMetadata {
                filename: filename.clone(),
                artifact_type: Self::infer_artifact_type(&filename),
                is_binary,
                version: Self::parse_version(&filename),
                source_path: path.to_path_buf(),
                last_modified,
            });
        }
        
        Ok(artifacts)
    }
    
    fn get_content(&self, reference: &ContentRef) -> Result<String> {
        fs::read_to_string(&reference.source_path)
            .context("Failed to read artifact content")
    }
}
