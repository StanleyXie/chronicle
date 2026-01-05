//! Extract command implementation

use anyhow::Result;

use crate::probe::ProbeRegistry;
use crate::store::MetadataStore;

pub fn run(store: &MetadataStore, registry: &ProbeRegistry) -> Result<()> {
    println!("Discovering available probes...\n");

    let available = registry.available_probes();

    if available.is_empty() {
        println!("No probes available. Check your configuration.");
        return Ok(());
    }

    for probe in available {
        println!("📡 {} ({})", probe.id(), probe.description());

        // Ensure provider exists (for multi-provider sources, we'll store specific ones at message level)
        if probe.source_type() == crate::probe::SourceType::Single {
            store.ensure_provider(probe.provider(), probe.provider(), None)?;
        }

        // Ensure probe source exists
        store.ensure_probe_source(
            probe.id(),
            if probe.source_type() == crate::probe::SourceType::Single {
                Some(probe.provider())
            } else {
                None
            },
            probe.source(),
            probe.source_type(),
            None, // base_path not tracked in DB yet
            "active",
        )?;

        // Discover sessions
        let sessions = probe.discover()?;
        println!("   Found {} sessions", sessions.len());

        for session in &sessions {
            print!("   → {} ", &session.id[..8.min(session.id.len())]);

            // Extract metadata
            let metadata = probe.extract_metadata(session)?;

            // Store session
            let session_id = store.upsert_session(probe.id(), session, &metadata)?;

            // Store messages
            if !metadata.messages.is_empty() {
                store.insert_messages(&session_id, &metadata.messages)?;
                print!("({} msgs) ", metadata.messages.len());
            }

            if let Some(ref title) = metadata.title {
                let display_title = if title.len() > 30 {
                    format!("{}...", &title[..27])
                } else {
                    title.clone()
                };
                print!("- {}", display_title);
            }

            println!();
        }

        store.update_probe_indexed(probe.id())?;
        println!();
    }

    println!("✅ Extraction complete!");
    Ok(())
}
