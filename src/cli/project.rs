use anyhow::Result;
use crate::store::MetadataStore;
use uuid::Uuid;

pub fn create(
    store: &MetadataStore,
    name: String,
    project_type: String,
    path: Option<String>,
) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    store.create_project(&id, &name, &project_type, path.as_deref(), None)?;
    println!("Project '{}' created with ID: {}", name, id);
    Ok(())
}

pub fn list(store: &MetadataStore) -> Result<()> {
    let projects = store.list_projects()?;
    if projects.is_empty() {
        println!("No projects found.");
        return Ok(());
    }

    println!("{:<12} {:<20} {:<10} {:<8} {:<30}", "ID", "Name", "Type", "Sessions", "Path");
    println!("{}", "-".repeat(85));
    for p in projects {
        println!(
            "{:<12} {:<20} {:<10} {:<8} {:<30}",
            &p.id[..8],
            p.name,
            p.project_type,
            p.session_count,
            p.primary_path.unwrap_or_default()
        );
    }
    Ok(())
}

pub fn add_path(store: &MetadataStore, project_id_query: String, path: String) -> Result<()> {
    // Find project by id or name
    let projects = store.list_projects()?;
    let project = projects.iter().find(|p| p.id.starts_with(&project_id_query) || p.name == project_id_query)
        .ok_or_else(|| anyhow::anyhow!("Project not found: {}", project_id_query))?;

    store.add_project_path(&project.id, &path, false)?;
    println!("Added path '{}' to project '{}'", path, project.name);
    Ok(())
}

pub fn add_git(store: &MetadataStore, project_id_query: String, remote: String) -> Result<()> {
    let projects = store.list_projects()?;
    let project = projects.iter().find(|p| p.id.starts_with(&project_id_query) || p.name == project_id_query)
        .ok_or_else(|| anyhow::anyhow!("Project not found: {}", project_id_query))?;

    store.add_project_identifier(&project.id, "git_remote", &remote)?;
    println!("Added git remote '{}' to project '{}'", remote, project.name);
    Ok(())
}
