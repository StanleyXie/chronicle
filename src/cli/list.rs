//! List command implementation

use anyhow::Result;

use crate::store::MetadataStore;

pub fn run(store: &MetadataStore, provider: Option<String>, source: Option<String>) -> Result<()> {
    let sessions = store.list_sessions(provider.as_deref(), source.as_deref())?;

    if sessions.is_empty() {
        println!("No sessions found. Run 'chronicle extract' first.");
        return Ok(());
    }

    println!(
        "{:<12} {:<10} {:<12} {:<12} {:<15} {}",
        "Timestamp", "ID", "Project", "Provider", "Source", "Title"
    );
    println!("{}", "-".repeat(100));

    for session in sessions {
        // Format timestamp
        let timestamp = session
            .first_timestamp
            .as_ref()
            .map(|ts| {
                if ts.len() >= 16 {
                    format!("{} {}", &ts[5..10], &ts[11..16])
                } else {
                    ts.clone()
                }
            })
            .unwrap_or_else(|| "-".to_string());

        // Project name
        let project = session.project_name.as_deref().unwrap_or("-");

        // Truncate title
        let title = session
            .title
            .as_ref()
            .map(|t| {
                let t = t.lines().next().unwrap_or(t);
                if t.len() > 35 {
                    format!("{}...", &t[..32])
                } else {
                    t.to_string()
                }
            })
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<12} {:<10} {:<12} {:<12} {:<15} {}",
            timestamp,
            session.short_hash,
            project,
            session.provider_name,
            session.source_name,
            title,
        );
    }

    Ok(())
}
