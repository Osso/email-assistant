mod support;

use crate::classifier::{Classification, Classifier};
use crate::config::Config;
use crate::labels::LabelManager;
use crate::learning::{is_system_label, Correction, LearningEngine};
use crate::predictions::PredictionStore;
use crate::profile::Profile;
use crate::providers::gmail::GmailProvider;
use crate::providers::outlook::OutlookProvider;
use crate::providers::outlook_web::OutlookWebProvider;
use crate::providers::{Email, EmailProvider};
use crate::rules;
use anyhow::Result;
use support::{
    build_status_indicators, learn_from_manual_action, print_action_preview,
    print_needs_reply_entry, run_summary_prompt, summary_prompt,
};

const CORRECTION_BATCH_SIZE: usize = 25;
const INBOX_CLASSIFICATION_QUERY: &str = "-label:Classified";
const ARCHIVED_CLASSIFICATION_QUERY: &str = "-label:Classified -in:spam -in:trash";

struct CorrectionPass {
    deleted_ids: Vec<String>,
    corrected_ids: Vec<String>,
    had_corrections: bool,
}

async fn create_provider(name: &str) -> Result<Box<dyn EmailProvider>> {
    match name {
        "gmail" => Ok(Box::new(GmailProvider::new().await?)),
        "outlook" => Ok(Box::new(OutlookProvider::new().await?)),
        "outlook-web" => Ok(Box::new(OutlookWebProvider::new()?)),
        _ => anyhow::bail!(
            "Unknown provider: {}. Use 'gmail', 'outlook', or 'outlook-web'",
            name
        ),
    }
}

pub async fn config(provider: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;

    if let Some(provider) = provider {
        validate_provider_name(&provider)?;
        cfg.provider = Some(provider.clone());
        cfg.save()?;
        println!("Default provider set to: {}", provider);
        return Ok(());
    }

    println!("Current settings:");
    println!("  provider: {}", cfg.default_provider());
    Ok(())
}

pub async fn login(provider_name: &str) -> Result<()> {
    match provider_name {
        "gmail" => login_gmail().await,
        "outlook" => login_outlook().await,
        "outlook-web" => {
            println!("outlook-web uses browser automation - no login required.");
            println!("Start your browser with remote debugging enabled:");
            println!("  vivaldi --remote-debugging-port=9222");
            println!("Then open Outlook Web and log in manually.");
            Ok(())
        }
        _ => anyhow::bail!(
            "Unknown provider: {}. Use 'gmail', 'outlook', or 'outlook-web'",
            provider_name
        ),
    }
}

pub async fn scan(max: u32, dry_run: bool, provider_name: &str, archived: bool) -> Result<()> {
    let _config = Config::load()?;
    let provider = create_provider(provider_name).await?;
    let mut profile = Profile::load()?;
    let mut predictions = PredictionStore::load()?;
    let _label_manager = LabelManager::load()?;

    let correction_pass =
        run_scan_correction_pass(provider.as_ref(), &mut profile, &predictions, dry_run).await?;
    persist_profile(&profile, correction_pass.had_corrections, dry_run)?;
    remove_predictions(
        &mut predictions,
        &correction_pass.deleted_ids,
        &correction_pass.corrected_ids,
        dry_run,
    );

    let classifier = Classifier::new(&profile);
    let user_rules = rules::load_rules().unwrap_or_default();
    let emails = load_scan_emails(provider.as_ref(), max, archived).await?;

    for email in emails {
        process_scan_email(
            provider.as_ref(),
            &classifier,
            &user_rules,
            &mut predictions,
            email,
            dry_run,
        )
        .await?;
    }

    save_predictions(&predictions, dry_run)?;
    Ok(())
}

pub async fn labels_list(provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let label_manager = LabelManager::load()?;
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
        return Ok(());
    }

    print_label_cleanup_result(&removed, dry_run);
    if !dry_run {
        label_manager.save()?;
        profile.save()?;
    }

    Ok(())
}

pub async fn spam(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let mut profile = Profile::load()?;
    let predictions = PredictionStore::load()?;
    let email = provider.get_message(id).await?;

    if dry_run {
        print_action_preview("mark as spam", &email);
        return Ok(());
    }

    provider.mark_spam(id).await?;
    println!("Marked as spam: \"{}\"", email.subject);
    learn_from_manual_action(
        provider.as_ref(),
        &mut profile,
        &predictions,
        id,
        "spam",
        &email,
    )
    .await
}

pub async fn unspam(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let mut profile = Profile::load()?;
    let predictions = PredictionStore::load()?;
    let email = provider.get_message(id).await?;

    if dry_run {
        print_action_preview("remove from spam", &email);
        return Ok(());
    }

    provider.unspam(id).await?;
    println!("Removed from spam: \"{}\"", email.subject);
    learn_from_manual_action(
        provider.as_ref(),
        &mut profile,
        &predictions,
        id,
        "unspam",
        &email,
    )
    .await
}

pub async fn archive(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let email = provider.get_message(id).await?;

    if dry_run {
        print_action_preview("archive", &email);
        return Ok(());
    }

    provider.archive(id).await?;
    println!("Archived: \"{}\"", email.subject);
    Ok(())
}

pub async fn delete(id: &str, dry_run: bool, provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let email = provider.get_message(id).await?;

    if dry_run {
        print_action_preview("move to trash", &email);
        return Ok(());
    }

    provider.trash(id).await?;
    println!("Moved to trash: \"{}\"", email.subject);
    Ok(())
}

pub async fn label(id: &str, label: &str, dry_run: bool, provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let mut profile = Profile::load()?;
    let predictions = PredictionStore::load()?;
    let email = provider.get_message(id).await?;

    if dry_run {
        println!("Would add label '{}' to: \"{}\"", label, email.subject);
        println!("  From: {}", email.from);
        return Ok(());
    }

    provider.add_label(id, label).await?;
    println!("Added label '{}' to: \"{}\"", label, email.subject);

    let action = format!("label:{}", label);
    learn_from_manual_action(
        provider.as_ref(),
        &mut profile,
        &predictions,
        id,
        &action,
        &email,
    )
    .await
}

pub async fn learn(dry_run: bool, provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let mut profile = Profile::load()?;
    let mut predictions = PredictionStore::load()?;
    let correction_pass =
        run_learning_pass(provider.as_ref(), &mut profile, &predictions, dry_run).await?;

    if correction_pass.had_corrections && !dry_run {
        profile.save()?;
        println!("Profile updated.");
    }

    cleanup_deleted_predictions(&mut predictions, &correction_pass.deleted_ids, dry_run)?;
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

    for prediction in predictions
        .all_predictions()
        .filter(|prediction| prediction.needs_reply())
    {
        let Ok(email) = provider.get_message(&prediction.email_id).await else {
            continue;
        };

        print_needs_reply_entry(prediction, &email);
        found = true;
    }

    if !found {
        println!("No emails need a reply.");
    }

    Ok(())
}

pub async fn summary(provider_name: &str) -> Result<()> {
    let provider = create_provider(provider_name).await?;
    let emails = provider
        .list_messages(100, "INBOX", Some(INBOX_CLASSIFICATION_QUERY))
        .await?;

    if emails.is_empty() {
        println!("No unclassified emails in inbox.");
        return Ok(());
    }

    println!("Analyzing {} emails...\n", emails.len());
    let prompt = summary_prompt(&emails);
    let response = run_summary_prompt(&prompt).await?;
    println!("{}", response.trim());
    Ok(())
}

fn validate_provider_name(provider: &str) -> Result<()> {
    match provider {
        "gmail" | "outlook" | "outlook-web" => Ok(()),
        _ => anyhow::bail!(
            "Invalid provider: {}. Use 'gmail', 'outlook', or 'outlook-web'",
            provider
        ),
    }
}

async fn login_gmail() -> Result<()> {
    let cfg = gmail::config::load_config()?;
    let client_id = cfg.client_id();
    let client_secret = cfg.client_secret();
    gmail::auth::login(client_id, client_secret).await?;
    println!("Gmail login successful! Tokens saved.");
    Ok(())
}

async fn login_outlook() -> Result<()> {
    let cfg = outlook::config::load_config()?;
    let client_id = cfg.client_id();
    outlook::auth::login(client_id).await?;
    println!("Outlook login successful! Tokens saved.");
    Ok(())
}

async fn run_scan_correction_pass(
    provider: &dyn EmailProvider,
    profile: &mut Profile,
    predictions: &PredictionStore,
    dry_run: bool,
) -> Result<CorrectionPass> {
    let mut learning = LearningEngine::new(provider, profile, predictions);
    let result = learning.detect_corrections().await?;
    let corrected_ids = result
        .corrections
        .iter()
        .map(|correction| correction.email_id.clone())
        .collect::<Vec<_>>();
    let had_corrections = !result.corrections.is_empty();

    if had_corrections {
        print_scan_corrections(&result.corrections);
        if dry_run {
            println!("  [dry-run] Would update profile with corrections");
        } else {
            apply_corrections_in_batches(&mut learning, &result.corrections, true).await?;
        }
    }

    Ok(CorrectionPass {
        deleted_ids: result.deleted_ids,
        corrected_ids,
        had_corrections,
    })
}

async fn run_learning_pass(
    provider: &dyn EmailProvider,
    profile: &mut Profile,
    predictions: &PredictionStore,
    dry_run: bool,
) -> Result<CorrectionPass> {
    let mut learning = LearningEngine::new(provider, profile, predictions);
    let result = learning.detect_corrections().await?;
    let had_corrections = !result.corrections.is_empty();

    if !had_corrections {
        println!("No corrections found.");
        return Ok(CorrectionPass {
            deleted_ids: result.deleted_ids,
            corrected_ids: Vec::new(),
            had_corrections: false,
        });
    }

    print_learning_corrections(&result.corrections);
    if dry_run {
        println!("\n[dry-run] Would update profile with these corrections");
    } else {
        apply_corrections_in_batches(&mut learning, &result.corrections, false).await?;
    }

    Ok(CorrectionPass {
        deleted_ids: result.deleted_ids,
        corrected_ids: Vec::new(),
        had_corrections: true,
    })
}

async fn apply_corrections_in_batches<P: EmailProvider + ?Sized>(
    learning: &mut LearningEngine<'_, P>,
    corrections: &[Correction],
    continue_on_error: bool,
) -> Result<()> {
    let chunks = corrections
        .chunks(CORRECTION_BATCH_SIZE)
        .collect::<Vec<_>>();

    for (index, chunk) in chunks.iter().enumerate() {
        print_correction_batch_status(index, chunks.len());

        if continue_on_error {
            if let Err(error) = learning.apply_corrections(chunk).await {
                eprintln!("  Warning: profile update failed: {}", error);
                eprintln!("  Continuing with classification...");
            }
            continue;
        }

        learning.apply_corrections(chunk).await?;
    }

    Ok(())
}

fn print_correction_batch_status(index: usize, total_batches: usize) {
    if total_batches > 1 {
        println!(
            "Updating profile (batch {}/{})...",
            index + 1,
            total_batches
        );
    } else {
        println!("Updating profile...");
    }
}

fn print_scan_corrections(corrections: &[Correction]) {
    println!("Found {} corrections:", corrections.len());
    for correction in corrections {
        println!(
            "  - {} | {} (predicted: {:?}, actual: {:?})",
            correction.email_id,
            correction.subject.chars().take(40).collect::<String>(),
            correction.predicted_labels,
            correction.actual_labels
        );
    }
}

fn print_learning_corrections(corrections: &[Correction]) {
    println!("Found {} corrections:", corrections.len());
    for correction in corrections {
        println!(
            "  - {} (predicted: {:?}, actual: {:?})",
            correction.email_id, correction.predicted_labels, correction.actual_labels
        );
    }
}

fn persist_profile(profile: &Profile, had_corrections: bool, dry_run: bool) -> Result<()> {
    if had_corrections && !dry_run {
        profile.save()?;
    }
    Ok(())
}

fn remove_predictions(
    predictions: &mut PredictionStore,
    deleted_ids: &[String],
    corrected_ids: &[String],
    dry_run: bool,
) {
    if dry_run {
        return;
    }

    for id in deleted_ids {
        predictions.remove(id);
    }
    for id in corrected_ids {
        predictions.remove(id);
    }
}

async fn load_scan_emails(
    provider: &dyn EmailProvider,
    max: u32,
    archived: bool,
) -> Result<Vec<Email>> {
    let label = if archived { "" } else { "INBOX" };
    let query = if archived {
        ARCHIVED_CLASSIFICATION_QUERY
    } else {
        INBOX_CLASSIFICATION_QUERY
    };

    provider.list_messages(max, label, Some(query)).await
}

async fn process_scan_email(
    provider: &dyn EmailProvider,
    classifier: &Classifier<'_>,
    user_rules: &[rules::Rule],
    predictions: &mut PredictionStore,
    email: Email,
    dry_run: bool,
) -> Result<()> {
    let mut classification = classifier.classify(&email).await?;
    rules::apply_rules(&email, &mut classification, user_rules);
    protect_personal_and_reply_emails(&mut classification);

    print_scan_result(&email, &classification);
    if dry_run {
        print_scan_dry_run(&classification);
        return Ok(());
    }

    apply_scan_actions(provider, predictions, &email, &classification).await
}

fn protect_personal_and_reply_emails(classification: &mut Classification) {
    if !classification.delete {
        return;
    }

    let is_personal = classification
        .theme
        .iter()
        .any(|theme| theme.eq_ignore_ascii_case("Personal"));
    let needs_reply = classification
        .action
        .iter()
        .any(|action| action.eq_ignore_ascii_case("Needs-Reply"));

    if is_personal || needs_reply {
        classification.delete = false;
    }
}

fn print_scan_result(email: &Email, classification: &Classification) {
    let is_important = classification
        .action
        .iter()
        .any(|action| action == "Important" || action == "Urgent");
    let status = build_status_indicators(&email.labels, is_important);
    let action_suffix = action_suffix(classification);
    let labels = classification.labels();

    println!(
        "{} | {} | {:?}{}",
        status,
        email.subject.chars().take(60).collect::<String>(),
        labels,
        action_suffix
    );
}

fn action_suffix(classification: &Classification) -> &'static str {
    if classification.delete {
        " → DELETE"
    } else if classification.archive {
        " → archive"
    } else {
        ""
    }
}

fn print_scan_dry_run(classification: &Classification) {
    println!(
        "  [dry-run] Would apply labels: {:?}",
        classification.labels()
    );
    if classification.delete {
        println!("  [dry-run] Would DELETE");
        return;
    }
    if classification.archive {
        println!("  [dry-run] Would archive");
    }
}

async fn apply_scan_actions(
    provider: &dyn EmailProvider,
    predictions: &mut PredictionStore,
    email: &Email,
    classification: &Classification,
) -> Result<()> {
    if classification.delete {
        trash_email(provider, email).await;
        return Ok(());
    }

    let labels = classification.labels();
    add_predicted_labels(provider, email, &labels).await;
    store_classification_prediction(provider, predictions, email, classification).await?;
    archive_if_needed(provider, email, classification).await;
    Ok(())
}

async fn trash_email(provider: &dyn EmailProvider, email: &Email) {
    if let Err(error) = provider.trash(&email.id).await {
        eprintln!("  Warning: couldn't delete: {}", error);
    }
}

async fn add_predicted_labels(provider: &dyn EmailProvider, email: &Email, labels: &[String]) {
    for label in labels {
        if let Err(error) = provider.add_label(&email.id, label).await {
            eprintln!("  Warning: couldn't apply label '{}': {}", label, error);
        }
    }
}

async fn store_classification_prediction(
    provider: &dyn EmailProvider,
    predictions: &mut PredictionStore,
    email: &Email,
    classification: &Classification,
) -> Result<()> {
    match provider.add_label(&email.id, "Classified").await {
        Ok(_) => {
            let pre_existing = email
                .labels
                .iter()
                .filter(|label| !is_system_label(label))
                .cloned()
                .collect();
            predictions.store(
                &email.id,
                &email.from,
                &email.subject,
                classification,
                pre_existing,
            )?;
        }
        Err(error) => {
            eprintln!("  Warning: couldn't apply Classified label: {}", error);
        }
    }

    Ok(())
}

async fn archive_if_needed(
    provider: &dyn EmailProvider,
    email: &Email,
    classification: &Classification,
) {
    if !classification.archive {
        return;
    }

    if let Err(error) = provider.archive(&email.id).await {
        eprintln!("  Warning: couldn't archive: {}", error);
    }
}

fn save_predictions(predictions: &PredictionStore, dry_run: bool) -> Result<()> {
    if dry_run {
        println!("\n[dry-run] Would save predictions");
        return Ok(());
    }

    predictions.save()
}

fn print_label_cleanup_result(labels: &[String], dry_run: bool) {
    if dry_run {
        println!("Would remove {} labels:", labels.len());
    } else {
        println!("Removed {} labels:", labels.len());
    }

    for label in labels {
        println!("  - {}", label);
    }
}

fn cleanup_deleted_predictions(
    predictions: &mut PredictionStore,
    deleted_ids: &[String],
    dry_run: bool,
) -> Result<()> {
    if deleted_ids.is_empty() {
        return Ok(());
    }

    println!(
        "Cleaned up {} deleted emails from predictions.",
        deleted_ids.len()
    );

    if dry_run {
        return Ok(());
    }

    for id in deleted_ids {
        predictions.remove(id);
    }
    predictions.save()
}
