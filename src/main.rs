mod classifier;
mod commands;
mod config;
mod labels;
mod learning;
mod predictions;
mod profile;
mod providers;
mod rules;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "email-assistant")]
#[command(about = "AI-powered email classification and learning assistant")]
struct Cli {
    /// Dry run mode - show what would happen without making changes
    #[arg(long, global = true)]
    dry_run: bool,

    /// Email provider to use (gmail, outlook, or outlook-web)
    #[arg(long, global = true)]
    provider: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure settings
    Config {
        /// Set default provider (gmail, outlook, or outlook-web)
        #[arg(long)]
        provider: Option<String>,
    },
    /// Authenticate with email provider (opens browser)
    Login,
    /// Scan and classify emails (learns from corrections first)
    Scan {
        /// Maximum number of emails to scan
        #[arg(short = 'n', long, default_value = "50")]
        max: u32,
        /// Scan archived emails instead of inbox
        #[arg(long)]
        archived: bool,
    },
    /// List all known labels
    Labels {
        #[command(subcommand)]
        action: Option<LabelsAction>,
    },
    /// Mark email as spam (triggers learning)
    Spam {
        /// Email ID
        id: String,
    },
    /// Remove email from spam (triggers learning)
    Unspam {
        /// Email ID
        id: String,
    },
    /// Archive email (remove from inbox, keep in All Mail)
    Archive {
        /// Email ID
        id: String,
    },
    /// Move email to trash
    Delete {
        /// Email ID
        id: String,
    },
    /// Add label to email (triggers learning)
    Label {
        /// Email ID
        id: String,
        /// Label to add
        label: String,
    },
    /// Detect and learn from user corrections
    Learn,
    /// Show current classification profile
    Profile,
    /// Show emails that need a reply
    NeedsReply,
    /// AI-generated inbox summary
    Summary,
}

#[derive(Subcommand)]
enum LabelsAction {
    /// Remove labels with no emails
    Cleanup,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::Config::load()?;
    let dry_run = cli.dry_run;
    let provider = selected_provider(&cli, &cfg).to_string();
    let command = cli.command;
    print_dry_run_notice(dry_run);
    run_command(command, dry_run, &provider).await
}

fn selected_provider<'a>(cli: &'a Cli, cfg: &'a config::Config) -> &'a str {
    cli.provider
        .as_deref()
        .unwrap_or_else(|| cfg.default_provider())
}

fn print_dry_run_notice(dry_run: bool) {
    if dry_run {
        println!("🔍 DRY RUN MODE - no changes will be made\n");
    }
}

async fn run_command(command: Commands, dry_run: bool, provider: &str) -> Result<()> {
    match command {
        Commands::Config {
            provider: new_provider,
        } => commands::config(new_provider).await,
        Commands::Login => commands::login(provider).await,
        Commands::Scan { max, archived } => commands::scan(max, dry_run, provider, archived).await,
        Commands::Labels { action } => run_labels_command(action, dry_run, provider).await,
        Commands::Spam { id } => commands::spam(&id, dry_run, provider).await,
        Commands::Unspam { id } => commands::unspam(&id, dry_run, provider).await,
        Commands::Archive { id } => commands::archive(&id, dry_run, provider).await,
        Commands::Delete { id } => commands::delete(&id, dry_run, provider).await,
        Commands::Label { id, label } => commands::label(&id, &label, dry_run, provider).await,
        Commands::Learn => commands::learn(dry_run, provider).await,
        Commands::Profile => commands::profile().await,
        Commands::NeedsReply => commands::needs_reply(provider).await,
        Commands::Summary => commands::summary(provider).await,
    }
}

async fn run_labels_command(
    action: Option<LabelsAction>,
    dry_run: bool,
    provider: &str,
) -> Result<()> {
    match action {
        Some(LabelsAction::Cleanup) => commands::labels_cleanup(dry_run, provider).await,
        None => commands::labels_list(provider).await,
    }
}
