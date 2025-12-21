mod classifier;
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

    /// Email provider to use (gmail or outlook)
    #[arg(long, global = true)]
    provider: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure settings
    Config {
        /// Set default provider (gmail or outlook)
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
    let dry_run = cli.dry_run;
    let cfg = config::Config::load()?;
    let provider = cli.provider.as_deref().unwrap_or_else(|| cfg.default_provider());

    if dry_run {
        println!("🔍 DRY RUN MODE - no changes will be made\n");
    }

    match cli.command {
        Commands::Config { provider: new_provider } => {
            commands::config(new_provider).await?;
        }
        Commands::Login => {
            commands::login(provider).await?;
        }
        Commands::Scan { max, archived } => {
            commands::scan(max, dry_run, provider, archived).await?;
        }
        Commands::Labels { action } => match action {
            Some(LabelsAction::Cleanup) => {
                commands::labels_cleanup(dry_run, provider).await?;
            }
            None => {
                commands::labels_list(provider).await?;
            }
        },
        Commands::Spam { id } => {
            commands::spam(&id, dry_run, provider).await?;
        }
        Commands::Unspam { id } => {
            commands::unspam(&id, dry_run, provider).await?;
        }
        Commands::Archive { id } => {
            commands::archive(&id, dry_run, provider).await?;
        }
        Commands::Delete { id } => {
            commands::delete(&id, dry_run, provider).await?;
        }
        Commands::Label { id, label } => {
            commands::label(&id, &label, dry_run, provider).await?;
        }
        Commands::Learn => {
            commands::learn(dry_run, provider).await?;
        }
        Commands::Profile => {
            commands::profile().await?;
        }
        Commands::NeedsReply => {
            commands::needs_reply(provider).await?;
        }
        Commands::Summary => {
            commands::summary(provider).await?;
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
    use crate::providers::outlook::OutlookProvider;
    use crate::providers::EmailProvider;
    use crate::rules;
    use anyhow::Result;

    async fn create_provider(name: &str) -> Result<Box<dyn EmailProvider>> {
        match name {
            "gmail" => Ok(Box::new(GmailProvider::new().await?)),
            "outlook" => Ok(Box::new(OutlookProvider::new().await?)),
            _ => anyhow::bail!("Unknown provider: {}. Use 'gmail' or 'outlook'", name),
        }
    }

    pub async fn config(provider: Option<String>) -> Result<()> {
        let mut cfg = Config::load()?;

        if let Some(p) = provider {
            if p != "gmail" && p != "outlook" {
                anyhow::bail!("Invalid provider: {}. Use 'gmail' or 'outlook'", p);
            }
            cfg.provider = Some(p.clone());
            cfg.save()?;
            println!("Default provider set to: {}", p);
        } else {
            println!("Current settings:");
            println!("  provider: {}", cfg.default_provider());
        }
        Ok(())
    }

    pub async fn login(provider_name: &str) -> Result<()> {
        match provider_name {
            "gmail" => {
                let cfg = gmail::config::load_config()?;
                let client_id = cfg.client_id();
                let client_secret = cfg.client_secret();
                gmail::auth::login(client_id, client_secret).await?;
                println!("Gmail login successful! Tokens saved.");
            }
            "outlook" => {
                let cfg = outlook::config::load_config()?;
                let client_id = cfg.client_id();
                outlook::auth::login(client_id).await?;
                println!("Outlook login successful! Tokens saved.");
            }
            _ => anyhow::bail!("Unknown provider: {}. Use 'gmail' or 'outlook'", provider_name),
        }
        Ok(())
    }

    pub async fn scan(max: u32, dry_run: bool, provider_name: &str, archived: bool) -> Result<()> {
        let _config = Config::load()?;
        let provider = create_provider(provider_name).await?;
        let mut profile = Profile::load()?;
        let mut predictions = PredictionStore::load()?;
        let _label_manager = LabelManager::load()?;

        // Step 1: Learn from corrections first
        let (deleted_ids, had_corrections) = {
            let mut learning = LearningEngine::new(&provider, &mut profile, &predictions);
            let result = learning.detect_corrections().await?;
            let had_corrections = !result.corrections.is_empty();

            if had_corrections {
                println!("Found {} corrections:", result.corrections.len());
                for correction in &result.corrections {
                    println!("  - {} | {} (predicted: {:?}, actual: {:?})",
                        correction.email_id,
                        correction.subject.chars().take(40).collect::<String>(),
                        correction.predicted_labels,
                        correction.actual_labels
                    );
                }
                if !dry_run {
                    // Batch corrections: update profile every 25 corrections
                    const BATCH_SIZE: usize = 25;
                    let chunks: Vec<_> = result.corrections.chunks(BATCH_SIZE).collect();
                    for (i, chunk) in chunks.iter().enumerate() {
                        if chunks.len() > 1 {
                            println!("Updating profile (batch {}/{})...", i + 1, chunks.len());
                        } else {
                            println!("Updating profile...");
                        }
                        if let Err(e) = learning.apply_corrections(chunk).await {
                            eprintln!("  Warning: profile update failed: {}", e);
                            eprintln!("  Continuing with classification...");
                        }
                    }
                } else {
                    println!("  [dry-run] Would update profile with corrections");
                }
            }

            (result.deleted_ids, had_corrections)
        };

        // Save profile after learning engine is dropped
        if had_corrections && !dry_run {
            profile.save()?;
        }

        // Clean up predictions for deleted emails
        if !dry_run {
            for id in &deleted_ids {
                predictions.remove(id);
            }
        }

        // Step 2: Classify new emails (exclude already classified)
        let classifier = Classifier::new(&profile);
        let user_rules = rules::load_rules().unwrap_or_default();
        let label = if archived { "" } else { "INBOX" };
        let emails = provider.list_messages(max, label, Some("-label:Classified")).await?;

        for email in emails {
            let mut classification = classifier.classify(&email).await?;

            // Apply user-defined rules (e.g., auto-delete based on To field)
            rules::apply_rules(&email, &mut classification, &user_rules);

            // Build status indicators
            let is_important = classification.action.iter()
                .any(|a| a == "Important" || a == "Urgent");
            let status = build_status_indicators(&email.labels, is_important);
            let action_str = if classification.delete {
                " → DELETE"
            } else if classification.archive {
                " → archive"
            } else {
                ""
            };

            // Format: theme labels + action labels
            let all_labels = classification.labels();
            println!(
                "{} {} | {} | {:?}{}",
                email.id,
                status,
                email.subject.chars().take(50).collect::<String>(),
                all_labels,
                action_str
            );

            if !dry_run {
                if classification.delete {
                    // Auto-delete based on profile rules
                    if let Err(e) = provider.trash(&email.id).await {
                        eprintln!("  Warning: couldn't delete: {}", e);
                    }
                } else {
                    // Apply predicted labels to Gmail so user can recategorize
                    for label in &all_labels {
                        if let Err(e) = provider.add_label(&email.id, label).await {
                            eprintln!("  Warning: couldn't apply label '{}': {}", label, e);
                        }
                    }
                    // Mark as classified so it won't be processed again
                    // Only store prediction if Classified label was successfully applied
                    match provider.add_label(&email.id, "Classified").await {
                        Ok(_) => {
                            predictions.store(&email.id, &email.from, &email.subject, &classification)?;
                        }
                        Err(e) => {
                            eprintln!("  Warning: couldn't apply Classified label: {}", e);
                        }
                    }
                    // Auto-archive if suggested
                    if classification.archive {
                        if let Err(e) = provider.archive(&email.id).await {
                            eprintln!("  Warning: couldn't archive: {}", e);
                        }
                    }
                }
            } else {
                println!("  [dry-run] Would apply labels: {:?}", all_labels);
                if classification.delete {
                    println!("  [dry-run] Would DELETE");
                } else if classification.archive {
                    println!("  [dry-run] Would archive");
                }
            }
        }

        if !dry_run {
            predictions.save()?;
        } else {
            println!("\n[dry-run] Would save predictions");
        }
        Ok(())
    }

    pub async fn labels_list(provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
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

    pub async fn labels_cleanup(dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
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

    pub async fn spam(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
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

    pub async fn unspam(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
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

    pub async fn archive(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;

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

    pub async fn delete(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;

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

    pub async fn label(id: &str, label: &str, dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
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

    pub async fn learn(dry_run: bool, provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
        let mut profile = Profile::load()?;
        let mut predictions = PredictionStore::load()?;

        let (deleted_ids, had_corrections) = {
            let mut learning = LearningEngine::new(&provider, &mut profile, &predictions);
            let result = learning.detect_corrections().await?;
            let had_corrections = !result.corrections.is_empty();

            if result.corrections.is_empty() {
                println!("No corrections found.");
            } else {
                println!("Found {} corrections:", result.corrections.len());
                for correction in &result.corrections {
                    println!("  - {} (predicted: {:?}, actual: {:?})",
                        correction.email_id,
                        correction.predicted_labels,
                        correction.actual_labels
                    );
                }

                if dry_run {
                    println!("\n[dry-run] Would update profile with these corrections");
                } else {
                    // Batch corrections: update profile every 25 corrections
                    const BATCH_SIZE: usize = 25;
                    let chunks: Vec<_> = result.corrections.chunks(BATCH_SIZE).collect();
                    for (i, chunk) in chunks.iter().enumerate() {
                        if chunks.len() > 1 {
                            println!("\nUpdating profile (batch {}/{})...", i + 1, chunks.len());
                        } else {
                            println!("\nUpdating profile...");
                        }
                        learning.apply_corrections(chunk).await?;
                    }
                }
            }

            (result.deleted_ids, had_corrections)
        };

        // Save profile after learning engine is dropped
        if had_corrections && !dry_run {
            profile.save()?;
            println!("Profile updated.");
        }

        // Clean up predictions for deleted emails
        if !deleted_ids.is_empty() {
            println!("Cleaned up {} deleted emails from predictions.", deleted_ids.len());
            if !dry_run {
                for id in &deleted_ids {
                    predictions.remove(id);
                }
                predictions.save()?;
            }
        }

        Ok(())
    }

    pub async fn profile() -> Result<()> {
        let profile = Profile::load()?;
        println!("{}", profile.content());
        Ok(())
    }

    pub async fn needs_reply(provider_name: &str) -> Result<()> {
        let provider = create_provider(provider_name).await?;
        let predictions = PredictionStore::load()?;

        println!("Emails that need a reply:\n");

        let mut found = false;
        for prediction in predictions.all_predictions() {
            if prediction.needs_reply() {
                // Verify email still exists and get current state
                match provider.get_message(&prediction.email_id).await {
                    Ok(email) => {
                        let is_unread = email.labels.iter().any(|l| l == "UNREAD");
                        let marker = if is_unread { "●" } else { " " };
                        println!(
                            "{} {} | {} | {:?}",
                            marker,
                            prediction.email_id,
                            prediction.subject.chars().take(50).collect::<String>(),
                            prediction.all_labels()
                        );
                        found = true;
                    }
                    Err(_) => {
                        // Email was deleted, skip
                    }
                }
            }
        }

        if !found {
            println!("No emails need a reply.");
        }

        Ok(())
    }

    pub async fn summary(provider_name: &str) -> Result<()> {
        use anyhow::Context;
        use std::process::Stdio;
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command;
        use tokio::time::timeout;

        let provider = create_provider(provider_name).await?;

        // Fetch unclassified emails
        let emails = provider.list_messages(100, "INBOX", Some("-label:Classified")).await?;

        if emails.is_empty() {
            println!("No unclassified emails in inbox.");
            return Ok(());
        }

        println!("Analyzing {} emails...\n", emails.len());

        // Format emails for Claude
        let mut email_text = String::new();
        for (i, email) in emails.iter().enumerate() {
            let body_preview: String = email.body.chars().take(2000).collect();
            email_text.push_str(&format!(
                "=== Email {} ===\nFrom: {}\nSubject: {}\nBody:\n{}\n\n",
                i + 1,
                email.from,
                email.subject,
                body_preview
            ));
        }

        let prompt = format!(
            r#"Analyze these emails and provide actionable summary.

{}

Rules:
- Always identify emails by sender and subject, never "Email 1" or "Email 2"
- For emails needing reply: state WHAT to reply (e.g. "confirm attendance", "approve budget")
- For urgent items: state WHY it's urgent and WHAT action to take
- Skip generic notifications that need no action
- Be specific and actionable, not vague

Format:
## Needs Action
- [sender]: [subject] → [specific action needed]

## FYI (no action needed)
- [sender]: [subject] - [one line summary]"#,
            email_text
        );

        // Save prompt for debugging
        let prompt_file = std::env::temp_dir().join("email-assistant-summary-prompt.txt");
        let _ = std::fs::write(&prompt_file, &prompt);

        // Disallow all MCP tools to prevent prompt injection from email content
        let mut child = Command::new("claude")
            .args([
                "-p", "-",
                "--model", "haiku",
                "--disallowedTools", "mcp__browsermcp__browser_navigate,mcp__browsermcp__browser_click,mcp__browsermcp__browser_snapshot,mcp__browsermcp__browser_screenshot,mcp__browsermcp__browser_wait,mcp__browsermcp__browser_hover,mcp__browsermcp__browser_type,mcp__browsermcp__browser_select_option,mcp__browsermcp__browser_press_key,mcp__browsermcp__browser_go_back,mcp__browsermcp__browser_go_forward,mcp__browsermcp__browser_get_console_logs",
                "--no-session-persistence",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn claude CLI")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
        }

        let output = timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .context("Claude CLI timed out after 60s")?
            .context("Failed to run claude CLI")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude CLI failed: {}", stderr);
        }

        let response = String::from_utf8_lossy(&output.stdout);
        println!("{}", response.trim());

        Ok(())
    }

    /// Build status indicators from Gmail labels and classifier
    /// Returns a string like "[● *🔥]" for unread+starred+important
    fn build_status_indicators(labels: &[String], is_important: bool) -> String {
        let in_inbox = labels.iter().any(|l| l == "INBOX");
        let is_starred = labels.iter().any(|l| l == "STARRED");
        let is_unread = labels.iter().any(|l| l == "UNREAD");

        let c1 = if is_unread { "●" } else { " " };
        let c2 = if is_starred { "*" } else if !in_inbox { "A" } else { " " };
        let c3 = if is_important { "🔥" } else { " " };

        format!("[{}{}{}]", c1, c2, c3)
    }
}
