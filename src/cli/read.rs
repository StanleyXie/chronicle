//! Read command implementation

use anyhow::Result;
use serde_json::Value;

use crate::probe::{ContentRef, ProbeRegistry};
use crate::store::MetadataStore;

pub fn run(
    store: &MetadataStore,
    registry: &ProbeRegistry,
    session_id: &str,
    full: bool,
    tools: bool,
) -> Result<()> {
    let session = store.get_session(session_id)?;

    let session = match session {
        Some(s) => s,
        None => {
            println!("Session '{}' not found.", session_id);
            return Ok(());
        }
    };

    println!("\n{}", "=".repeat(80));
    println!("Session: {} ({})", session.short_hash, session.external_id);
    println!(
        "Provider: {} | Source: {}",
        session.provider_name, session.source_name
    );
    if let Some(model) = &session.primary_model {
        println!("Primary Model: {}", model);
    }
    if let Some(project) = &session.project_name {
        println!("Project: {}", project);
    } else if let Some(path) = &session.project_path {
        println!("Raw Path: {}", path);
    }
    println!("{}", "=".repeat(80));

    // Show messages
    let messages = store.get_messages(&session.id)?;

    if messages.is_empty() {
        println!("\nNo messages found (this may be an empty session).");
        return Ok(());
    }

    let probe = registry.get_probe(&session.probe_source_id);

    for msg in messages {
        let provider_info = if let Some(p) = &msg.provider_id {
            format!(" | {}", p)
        } else {
            String::new()
        };
        let model_info = if let Some(m) = &msg.model {
            format!(" | {}", m)
        } else {
            String::new()
        };

        println!(
            "\n[{}{}{}] ({})",
            msg.role.to_uppercase(),
            provider_info,
            model_info,
            msg.timestamp.as_deref().unwrap_or("?")
        );

        if full {
            if let Some(probe) = probe {
                let content_ref = ContentRef {
                    source_path: msg.source_path.into(),
                    byte_offset: msg.byte_offset.map(|o| o as u64),
                    line_number: msg.line_number.map(|n| n as u32),
                    content_path: msg.content_ref.map(Into::into),
                };

                match probe.get_content(&content_ref) {
                    Ok(raw) => {
                        // For JSONL sources, we might need to parse and extract content
                        // For OpenCode, get_content already returns the extracted text
                        if raw.trim().starts_with('{') {
                            if let Ok(json) = serde_json::from_str::<Value>(&raw) {
                                if let Some(content) =
                                    json.get("message").and_then(|m| m.get("content"))
                                {
                                    print_content(content);
                                } else if let Some(content) = json.get("content") {
                                    print_content(content);
                                } else {
                                    println!("{}", raw);
                                }
                            } else {
                                println!("{}", raw);
                            }
                        } else {
                            println!("{}", raw);
                        }
                    }
                    Err(e) => println!("[Error loading content: {}]", e),
                }
            }
        } else {
            println!("[Use --full to see content]");
        }

        if tools && msg.has_tool_use {
            println!("  🔧 Has tool use");
        }

        println!("{}", "-".repeat(40));
    }

    Ok(())
}

fn print_content(content: &Value) {
    match content {
        Value::String(s) => println!("{}", s),
        Value::Array(arr) => {
            for item in arr {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    println!("{}", text);
                } else if item.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                        println!("  🔧 [Tool: {}]", name);
                    }
                } else if item.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    if let Some(thinking) = item.get("thinking").and_then(|t| t.as_str()) {
                        println!("  💭 [Thinking]\n{}", thinking);
                    }
                }
            }
        }
        _ => println!("{}", content),
    }
}
