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

    match cli.command {
        Commands::Scan { max } => {
            commands::scan(max).await?;
        }
        Commands::Labels { action } => match action {
            Some(LabelsAction::Cleanup) => {
                commands::labels_cleanup().await?;
            }
            None => {
                commands::labels_list().await?;
            }
        },
        Commands::Spam { id } => {
            commands::spam(&id).await?;
        }
        Commands::Unspam { id } => {
            commands::unspam(&id).await?;
        }
        Commands::Delete { id } => {
            commands::delete(&id).await?;
        }
        Commands::Label { id, label } => {
            commands::label(&id, &label).await?;
        }
        Commands::Learn => {
            commands::learn().await?;
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

    pub async fn scan(max: u32) -> Result<()> {
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
            learning.apply_corrections(&corrections).await?;
            profile.save()?;
        }

        // Step 2: Classify new emails
        let classifier = Classifier::new(&profile);
        let emails = provider.list_messages(max, "INBOX").await?;

        for email in emails {
            if predictions.has_prediction(&email.id) {
                continue; // Already classified
            }

            let classification = classifier.classify(&email).await?;
            println!(
                "{} | {} | {:?}",
                email.id,
                email.subject.chars().take(50).collect::<String>(),
                classification.labels
            );

            predictions.store(&email.id, &classification)?;
        }

        predictions.save()?;
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

    pub async fn labels_cleanup() -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut label_manager = LabelManager::load()?;
        let mut profile = Profile::load()?;

        let removed = label_manager.cleanup(&provider, &mut profile).await?;
        if removed.is_empty() {
            println!("No labels to clean up.");
        } else {
            println!("Removed {} labels:", removed.len());
            for label in removed {
                println!("  - {}", label);
            }
            label_manager.save()?;
            profile.save()?;
        }

        Ok(())
    }

    pub async fn spam(id: &str) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        // Execute action
        provider.mark_spam(id).await?;

        // Get email details for learning
        let email = provider.get_message(id).await?;
        println!("Marked as spam: \"{}\"", email.subject);

        // Learn from this action
        let learning = LearningEngine::new(&provider, &mut profile, &predictions);
        if let Some(update) = learning.learn_from_action(id, "spam", &email).await? {
            println!("\n📝 Profile updated:");
            println!("{}", update);
            profile.save()?;
        }

        Ok(())
    }

    pub async fn unspam(id: &str) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        // Execute action
        provider.unspam(id).await?;

        // Get email details for learning
        let email = provider.get_message(id).await?;
        println!("Removed from spam: \"{}\"", email.subject);

        // Learn from this action
        let learning = LearningEngine::new(&provider, &mut profile, &predictions);
        if let Some(update) = learning.learn_from_action(id, "unspam", &email).await? {
            println!("\n📝 Profile updated:");
            println!("{}", update);
            profile.save()?;
        }

        Ok(())
    }

    pub async fn delete(id: &str) -> Result<()> {
        let provider = GmailProvider::new().await?;

        provider.trash(id).await?;

        let email = provider.get_message(id).await?;
        println!("Moved to trash: \"{}\"", email.subject);

        Ok(())
    }

    pub async fn label(id: &str, label: &str) -> Result<()> {
        let provider = GmailProvider::new().await?;
        let mut profile = Profile::load()?;
        let predictions = PredictionStore::load()?;

        // Execute action
        provider.add_label(id, label).await?;

        // Get email details for learning
        let email = provider.get_message(id).await?;
        println!("Added label '{}' to: \"{}\"", label, email.subject);

        // Learn from this action
        let learning = LearningEngine::new(&provider, &mut profile, &predictions);
        if let Some(update) = learning.learn_from_action(id, &format!("label:{}", label), &email).await? {
            println!("\n📝 Profile updated:");
            println!("{}", update);
            profile.save()?;
        }

        Ok(())
    }

    pub async fn learn() -> Result<()> {
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

            println!("\nUpdating profile...");
            learning.apply_corrections(&corrections).await?;
            profile.save()?;
            println!("Profile updated.");
        }

        Ok(())
    }

    pub async fn profile() -> Result<()> {
        let profile = Profile::load()?;
        println!("{}", profile.content());
        Ok(())
    }
}
