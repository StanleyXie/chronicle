use anyhow::Result;
use clap::{Parser, Subcommand};

use chronicle::cli::{extract, list, project, read, session};
use chronicle::config::Config;
use chronicle::probe::ProbeRegistry;
use chronicle::store::MetadataStore;

#[derive(Parser)]
#[command(name = "chronicle")]
#[command(about = "AI conversation history extraction and observability tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Config file path
    #[arg(short, long, default_value = "chronicle.yaml")]
    config: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract metadata from all available probes
    Extract,

    /// List sessions
    List {
        /// Filter by provider (claude, gemini, etc.)
        #[arg(short, long)]
        provider: Option<String>,

        /// Filter by probe source
        #[arg(short, long)]
        source: Option<String>,
    },

    /// Read a session
    Read {
        /// Session ID (short hash or full ID)
        session_id: String,

        /// Show full content (lazy load from source)
        #[arg(long)]
        full: bool,

        /// Show tool uses
        #[arg(long)]
        tools: bool,
    },

    /// Project management
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },

    /// Session management
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Show statistics
    Stats,
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// Create a new project
    Create {
        /// Project name
        name: String,
        /// Project type (code, research, general)
        #[arg(long, default_value = "code")]
        project_type: String,
        /// Primary directory path
        #[arg(short, long)]
        path: Option<String>,
    },
    /// List all projects
    List,
    /// Add an additional path to a project
    AddPath {
        /// Project ID or Name
        project: String,
        /// Path to add
        path: String,
    },
    /// Add a git remote to a project
    AddGit {
        /// Project ID or Name
        project: String,
        /// Git remote URL
        remote: String,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// Assign a session to a project
    Assign {
        /// Session ID (short hash)
        session: String,
        /// Project ID or Name
        project: String,
    },
    /// Mark a session as explicitly unassigned
    Unassign {
        /// Session ID (short hash)
        session: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config
    let config = Config::load(&cli.config).unwrap_or_default();

    // Initialize store
    let store = MetadataStore::open(&config.database_path())?;

    // Initialize probe registry
    let registry = ProbeRegistry::new(&config);

    match cli.command {
        Commands::Extract => {
            extract::run(&store, &registry)?;
        }
        Commands::List { provider, source } => {
            list::run(&store, provider, source)?;
        }
        Commands::Read {
            session_id,
            full,
            tools,
        } => {
            read::run(&store, &registry, &session_id, full, tools)?;
        }
        Commands::Project { command } => match command {
            ProjectCommands::Create {
                name,
                project_type,
                path,
            } => {
                project::create(&store, name, project_type, path)?;
            }
            ProjectCommands::List => {
                project::list(&store)?;
            }
            ProjectCommands::AddPath { project, path } => {
                project::add_path(&store, project, path)?;
            }
            ProjectCommands::AddGit { project, remote } => {
                project::add_git(&store, project, remote)?;
            }
        },
        Commands::Session { command } => match command {
            SessionCommands::Assign { session, project } => {
                session::assign(&store, session, project)?;
            }
            SessionCommands::Unassign { session } => {
                session::unassign(&store, session)?;
            }
        },
        Commands::Stats => {
            println!("Stats not yet implemented");
        }
    }

    Ok(())
}
