use crate::store::MetadataStore;
use anyhow::Result;

pub fn assign(store: &MetadataStore, session_query: String, project_query: String) -> Result<()> {
    // Find session
    let session = store
        .get_session(&session_query)?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_query))?;

    // Find project
    let projects = store.list_projects()?;
    let project = projects
        .iter()
        .find(|p| p.id.starts_with(&project_query) || p.name == project_query)
        .ok_or_else(|| anyhow::anyhow!("Project not found: {}", project_query))?;

    store.assign_session_to_project(&session.id, Some(&project.id))?;
    println!(
        "Assigned session '{}' to project '{}'",
        session.short_hash, project.name
    );
    Ok(())
}

pub fn unassign(store: &MetadataStore, session_query: String) -> Result<()> {
    let session = store
        .get_session(&session_query)?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_query))?;

    store.unassign_session(&session.id)?;
    println!("Unassigned session '{}'", session.short_hash);
    Ok(())
}
