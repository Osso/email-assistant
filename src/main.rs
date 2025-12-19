mod classifier;
mod config;
mod labels;
mod learning;
mod predictions;
mod profile;
mod providers;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "email-assistant")]
#[command(about = "AI-powered email classification and learning assistant")]
struct Cli {
    /// Dry run mode - show what would happen without making changes
    #[arg(long, global = true)]
    dry_run: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan and classify emails (learns from corrections first)
    Scan {
        /// Maximum number of emails to scan
        #[arg(short = 'n', long, default_value = "50")]
        max: u32,
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
}

#[derive(Subcommand)]
enum LabelsAction {
    /// Remove labels with no emails
    Cleanup,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let dry_run = cli.dry_run;

    if dry_run {
        println!("🔍 DRY RUN MODE - no changes will be made\n");
    }

    match cli.command {
        Commands::Scan { max } => {
            commands::scan(max, dry_run).await?;
        }
        Commands::Labels { action } => match action {
            Some(LabelsAction::Cleanup) => {
                commands::labels_cleanup(dry_run).await?;
            }
            None => {
                commands::labels_list().await?;
            }
        },
        Commands::Spam { id } => {
            commands::spam(&id, dry_run).await?;
        }
        Commands::Unspam { id } => {
            commands::unspam(&id, dry_run).await?;
        }
        Commands::Archive { id } => {
            commands::archive(&id, dry_run).await?;
        }
        Commands::Delete { id } => {
            commands::delete(&id, dry_run).await?;
        }
        Commands::Label { id, label } => {
            commands::label(&id, &label, dry_run).await?;
        }
        Commands::Learn => {
            commands::learn(dry_run).await?;
        }
        Commands::Profile => {
            commands::profile().await?;
        }
    }

    Ok(())
}

mod commands {
    use crate::classifier::Classifier;
    use crate::config::Config;
    use crate::labels::LabelManager;
    use crate::learning::LearningEngine;
    use crate::predictions::PredictionStore;
    use crate::profile::Profile;
    use crate::providers::gmail::GmailProvider;
    use crate::providers::EmailProvider;
    use anyhow::Result;

    pub async fn scan(max: u32, dry_run: bool) -> Result<()> {
        let _config = Config::load()?;
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let mut predictions = PredictionStore::load()?;
        let _label_manager = LabelManager::load()?;

        // Step 1: Learn from corrections first
        let mut learning = LearningEngine::new(&provider, &mut profile, &predictions);
        let corrections = learning.detect_corrections().await?;
        if !corrections.is_empty() {
            println!("Found {} corrections, updating profile...", corrections.len());
            if !dry_run {
                learning.apply_corrections(&corrections).await?;
                profile.save()?;
            } else {
                println!("  [dry-run] Would update profile with corrections");
            }
        }

        // Step 2: Classify new emails
        let classifier = Classifier::new(&profile);
        let emails = provider.list_messages(max, "INBOX").await?;

        for email in emails {
            if predictions.has_prediction(&email.id) {
                continue; // Already classified
            }

            let classification = classifier.classify(&email).await?;

            // Build status indicators from Gmail labels
            let status = build_status_indicators(&email.labels);

            println!(
                "{} {} | {} | {:?}",
                email.id,
                status,
                email.subject.chars().take(50).collect::<String>(),
                classification.labels
            );

            if !dry_run {
                predictions.store(&email.id, &classification)?;
            }
        }

        if !dry_run {
            predictions.save()?;
        } else {
            println!("\n[dry-run] Would save predictions");
        }
        Ok(())
    }

    pub async fn labels_list() -> Result<()> {
        let provider = GmailProvider::new().await?;
        let label_manager = LabelManager::load()?;

        // Fetch provider labels
        let provider_labels = provider.list_labels().await?;

        println!("Provider labels:");
        for label in &provider_labels {
            println!("  {} ({})", label.name, label.id);
        }

        println!("\nLLM-created labels:");
        for label in label_manager.llm_labels() {
            println!("  {}", label);
        }

        Ok(())
    }

    pub async fn labels_cleanup(dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut label_manager = LabelManager::load()?;
        let mut profile = Profile::load()?;

        let removed = label_manager.cleanup(&provider, &mut profile).await?;
        if removed.is_empty() {
            println!("No labels to clean up.");
        } else {
            if dry_run {
                println!("Would remove {} labels:", removed.len());
            } else {
                println!("Removed {} labels:", removed.len());
            }
            for label in removed {
                println!("  - {}", label);
            }
            if !dry_run {
                label_manager.save()?;
                profile.save()?;
            }
        }

        Ok(())
    }

    pub async fn spam(id: &str, dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        // Get email details first
        let email = provider.get_message(id).await?;

        if dry_run {
            println!("Would mark as spam: \"{}\"", email.subject);
            println!("  From: {}", email.from);
        } else {
            // Execute action
            provider.mark_spam(id).await?;
            println!("Marked as spam: \"{}\"", email.subject);

            // Learn from this action
            let learning = LearningEngine::new(&provider, &mut profile, &predictions);
            if let Some(update) = learning.learn_from_action(id, "spam", &email).await? {
                println!("\n📝 Profile updated:");
                println!("{}", update);
                profile.save()?;
            }
        }

        Ok(())
    }

    pub async fn unspam(id: &str, dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        // Get email details first
        let email = provider.get_message(id).await?;

        if dry_run {
            println!("Would remove from spam: \"{}\"", email.subject);
            println!("  From: {}", email.from);
        } else {
            // Execute action
            provider.unspam(id).await?;
            println!("Removed from spam: \"{}\"", email.subject);

            // Learn from this action
            let learning = LearningEngine::new(&provider, &mut profile, &predictions);
            if let Some(update) = learning.learn_from_action(id, "unspam", &email).await? {
                println!("\n📝 Profile updated:");
                println!("{}", update);
                profile.save()?;
            }
        }

        Ok(())
    }

    pub async fn archive(id: &str, dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;

        // Get email details first
        let email = provider.get_message(id).await?;

        if dry_run {
            println!("Would archive: \"{}\"", email.subject);
            println!("  From: {}", email.from);
        } else {
            provider.archive(id).await?;
            println!("Archived: \"{}\"", email.subject);
        }

        Ok(())
    }

    pub async fn delete(id: &str, dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;

        // Get email details first
        let email = provider.get_message(id).await?;

        if dry_run {
            println!("Would move to trash: \"{}\"", email.subject);
            println!("  From: {}", email.from);
        } else {
            provider.trash(id).await?;
            println!("Moved to trash: \"{}\"", email.subject);
        }

        Ok(())
    }

    pub async fn label(id: &str, label: &str, dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        // Get email details first
        let email = provider.get_message(id).await?;

        if dry_run {
            println!("Would add label '{}' to: \"{}\"", label, email.subject);
            println!("  From: {}", email.from);
        } else {
            // Execute action
            provider.add_label(id, label).await?;
            println!("Added label '{}' to: \"{}\"", label, email.subject);

            // Learn from this action
            let learning = LearningEngine::new(&provider, &mut profile, &predictions);
            if let Some(update) = learning.learn_from_action(id, &format!("label:{}", label), &email).await? {
                println!("\n📝 Profile updated:");
                println!("{}", update);
                profile.save()?;
            }
        }

        Ok(())
    }

    pub async fn learn(dry_run: bool) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        let mut learning = LearningEngine::new(&provider, &mut profile, &predictions);
        let corrections = learning.detect_corrections().await?;

        if corrections.is_empty() {
            println!("No corrections found.");
        } else {
            println!("Found {} corrections:", corrections.len());
            for correction in &corrections {
                println!("  - {} (predicted: {:?}, actual: {:?})",
                    correction.email_id,
                    correction.predicted_labels,
                    correction.actual_labels
                );
            }

            if dry_run {
                println!("\n[dry-run] Would update profile with these corrections");
            } else {
                println!("\nUpdating profile...");
                learning.apply_corrections(&corrections).await?;
                profile.save()?;
                println!("Profile updated.");
            }
        }

        Ok(())
    }

    pub async fn profile() -> Result<()> {
        let profile = Profile::load()?;
        println!("{}", profile.content());
        Ok(())
    }

    /// Build status indicators from Gmail labels
    /// Returns a string like "[*I]" for starred+important, "[ A]" for archived
    fn build_status_indicators(labels: &[String]) -> String {
        let in_inbox = labels.iter().any(|l| l == "INBOX");
        let is_starred = labels.iter().any(|l| l == "STARRED");
        let is_important = labels.iter().any(|l| l == "IMPORTANT");
        let is_unread = labels.iter().any(|l| l == "UNREAD");

        let c1 = if is_unread { '●' } else { ' ' };
        let c2 = if is_starred { '*' } else if !in_inbox { 'A' } else { ' ' };
        let c3 = if is_important { 'I' } else { ' ' };

        format!("[{}{}{}]", c1, c2, c3)
    }
}
