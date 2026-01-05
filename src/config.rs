//! Configuration management with YAML support

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub database: DatabaseConfig,

    #[serde(default)]
    pub probes: HashMap<String, ProbeConfig>,

    #[serde(default)]
    pub linking: LinkingConfig,

    #[serde(default)]
    pub deduplication: DeduplicationConfig,
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_path")]
    pub path: String,
}

/// Individual probe configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default)]
    pub status: Option<String>, // 'active', 'frozen', 'deprecated'

    #[serde(default)]
    pub base_path: Option<String>,
}

/// Project linking configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkingConfig {
    #[serde(default = "default_enabled")]
    pub auto_link: bool,

    #[serde(default = "default_enabled")]
    pub use_git_remote: bool,

    #[serde(default = "default_enabled")]
    pub normalize_paths: bool,
}

/// Deduplication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeduplicationConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
}

// Default value functions
fn default_database_path() -> String {
    "~/.local/share/chronicle/chronicle.db".to_string()
}

fn default_enabled() -> bool {
    true
}

fn default_confidence_threshold() -> f64 {
    0.8
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_database_path(),
        }
    }
}

impl Default for LinkingConfig {
    fn default() -> Self {
        Self {
            auto_link: true,
            use_git_remote: true,
            normalize_paths: true,
        }
    }
}

impl Default for DeduplicationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            confidence_threshold: default_confidence_threshold(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            probes: HashMap::new(),
            linking: LinkingConfig::default(),
            deduplication: DeduplicationConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from a YAML file
    /// Searches in order:
    /// 1. Provided path
    /// 2. ./chronicle.yaml (current directory)
    /// 3. ~/.config/chronicle/chronicle.yaml
    pub fn load(path: &str) -> Result<Self> {
        let search_paths = vec![
            shellexpand::tilde(path).to_string(),
            "chronicle.yaml".to_string(),
            shellexpand::tilde("~/.config/chronicle/chronicle.yaml").to_string(),
        ];

        for search_path in &search_paths {
            if std::path::Path::new(search_path).exists() {
                let content = std::fs::read_to_string(search_path)?;
                let config: Config = serde_yaml::from_str(&content)?;
                return Ok(config);
            }
        }

        // No config file found, use defaults
        Ok(Config::default())
    }

    /// Get the database path, expanding ~ to home directory
    pub fn database_path(&self) -> PathBuf {
        let expanded = shellexpand::tilde(&self.database.path).to_string();
        PathBuf::from(expanded)
    }

    /// Check if a probe is enabled
    /// Returns false if:
    /// - Probe is explicitly disabled
    /// - Probe status is 'frozen' or 'deprecated'
    pub fn is_probe_enabled(&self, probe_id: &str) -> bool {
        self.probes.get(probe_id).map_or(true, |p| {
            if !p.enabled {
                return false;
            }
            // Check status - frozen/deprecated probes are disabled
            match p.status.as_deref() {
                Some("frozen") | Some("deprecated") => false,
                _ => true,
            }
        })
    }

    /// Get the base path for a probe, if configured
    pub fn probe_path(&self, probe_id: &str) -> Option<PathBuf> {
        self.probes
            .get(probe_id)
            .and_then(|p| p.base_path.as_ref())
            .map(|p| PathBuf::from(shellexpand::tilde(p).to_string()))
    }

    /// Get probe status
    pub fn probe_status(&self, probe_id: &str) -> Option<&str> {
        self.probes
            .get(probe_id)
            .and_then(|p| p.status.as_deref())
    }

    /// List all configured probes
    pub fn list_probes(&self) -> Vec<(&str, &ProbeConfig)> {
        self.probes.iter().map(|(k, v)| (k.as_str(), v)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.linking.auto_link);
        assert!(config.deduplication.enabled);
        assert_eq!(config.deduplication.confidence_threshold, 0.8);
    }

    #[test]
    fn test_probe_enabled_with_frozen_status() {
        let mut config = Config::default();
        config.probes.insert(
            "test:Probe".to_string(),
            ProbeConfig {
                enabled: true,
                status: Some("frozen".to_string()),
                base_path: None,
            },
        );
        assert!(!config.is_probe_enabled("test:Probe"));
    }

    #[test]
    fn test_yaml_parsing() {
        let yaml = r#"
database:
  path: ~/.local/share/chronicle/test.db

probes:
  claude:ClaudeCode:
    enabled: true
    base_path: ~/.claude/projects
  gemini:Antigravity:
    enabled: false
    status: frozen

linking:
  auto_link: true
  use_git_remote: false
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.database.path, "~/.local/share/chronicle/test.db");
        assert!(config.is_probe_enabled("claude:ClaudeCode"));
        assert!(!config.is_probe_enabled("gemini:Antigravity"));
        assert!(!config.linking.use_git_remote);
    }
}
