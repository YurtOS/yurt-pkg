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
    Search { query: String },
    Info { name: String },
    Install { spec: String },
    Upgrade { names: Vec<String> },
    Remove { name: String },
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
        Command::Update => bail!("pkg update is deferred to the update-flow spec"),
        Command::Search { .. } | Command::Info { .. } => {
            bail!("pkg search/info require db.sqlite cache implementation")
        }
        Command::Install { .. } | Command::Upgrade { .. } => {
            bail!("install and upgrade planning are deferred to the resolver/installer spec")
        }
        Command::Remove { .. } => bail!("remove is deferred to the resolver/installer spec"),
        Command::List { .. } => bail!("list requires installed.sqlite implementation"),
        Command::AddRepo { .. } => bail!("add-repo requires repo:write capability integration"),
    }
}
