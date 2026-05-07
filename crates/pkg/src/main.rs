use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "pkg", about = "Yurt package client", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Update,
    Search {
        query: String,
    },
    Info {
        name: String,
    },
    Install {
        spec: String,
    },
    Upgrade {
        names: Vec<String>,
    },
    Remove {
        name: String,
    },
    List {
        #[arg(long)]
        yanked: bool,
    },
    AddRepo {
        url: String,
        #[arg(long)]
        signing_subject: String,
        #[arg(long)]
        signing_issuer: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        priority: Option<i64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md):
        // implement signed index fetch, freshness, rollback, and cache persistence.
        Command::Update => bail!("pkg update is deferred to the update-flow spec"),
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md):
        // implement db.sqlite-backed repository cache queries.
        Command::Search { .. } | Command::Info { .. } => {
            bail!("pkg search/info require db.sqlite cache implementation")
        }
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md):
        // implement dependency resolution, install planning, and atomic state updates.
        Command::Install { .. } | Command::Upgrade { .. } => {
            bail!("install and upgrade planning are deferred to the resolver/installer spec")
        }
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md):
        // implement installed-state mutation semantics.
        Command::Remove { .. } => bail!("remove is deferred to the resolver/installer spec"),
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md):
        // implement installed.sqlite reads.
        Command::List { .. } => bail!("list requires installed.sqlite implementation"),
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md):
        // implement trusted repo persistence and repo:write capability checks.
        Command::AddRepo { .. } => bail!("add-repo requires repo:write capability integration"),
    }
}
