use crate::predictions::{Prediction, PredictionStore};
use crate::profile::Profile;
use crate::providers::{Email, EmailProvider};
use anyhow::{Context, Result};
use chrono::Utc;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const CLAUDE_MODEL: &str = "haiku";
const CLAUDE_BROWSER_TOOLS: &str = "mcp__browsermcp__browser_navigate,mcp__browsermcp__browser_click,mcp__browsermcp__browser_snapshot,mcp__browsermcp__browser_screenshot,mcp__browsermcp__browser_wait,mcp__browsermcp__browser_hover,mcp__browsermcp__browser_type,mcp__browsermcp__browser_select_option,mcp__browsermcp__browser_press_key,mcp__browsermcp__browser_go_back,mcp__browsermcp__browser_go_forward,mcp__browsermcp__browser_get_console_logs";

#[derive(Debug)]
pub struct Correction {
    pub email_id: String,
    pub from: String,
    pub subject: String,
    pub predicted_labels: Vec<String>,
    pub actual_labels: Vec<String>,
    pub predicted_spam: bool,
    pub actual_spam: bool,
}

#[derive(Debug, Default)]
pub struct LearningResult {
    pub corrections: Vec<Correction>,
    pub deleted_ids: Vec<String>,
}

pub struct LearningEngine<'a, P: EmailProvider + ?Sized> {
    provider: &'a P,
    profile: &'a mut Profile,
    predictions: &'a PredictionStore,
}

impl<'a, P: EmailProvider + ?Sized> LearningEngine<'a, P> {
    pub fn new(
        provider: &'a P,
        profile: &'a mut Profile,
        predictions: &'a PredictionStore,
    ) -> Self {
        Self {
            provider,
            profile,
            predictions,
        }
    }

    pub async fn detect_corrections(&self) -> Result<LearningResult> {
        let mut result = LearningResult::default();

        for prediction in self.predictions.all_predictions() {
            let Some(email) = self
                .load_current_email(prediction, &mut result.deleted_ids)
                .await
            else {
                continue;
            };

            let actual_spam = email.labels.iter().any(|label| label == "SPAM");
            let label_mismatch = self.labels_changed(prediction, &email);
            if !label_mismatch && prediction.is_spam == actual_spam {
                continue;
            }

            result
                .corrections
                .push(build_correction(prediction, &email, actual_spam));
        }

        Ok(result)
    }

    pub async fn apply_corrections(&mut self, corrections: &[Correction]) -> Result<()> {
        if corrections.is_empty() {
            return Ok(());
        }

        let date = Utc::now().format("%Y-%m-%d").to_string();
        for correction in corrections {
            let description = describe_correction(&date, correction);
            self.profile.append_correction(&description);
        }

        let update = self.get_batched_profile_update(corrections).await?;
        if let Some(new_profile) = update {
            self.profile.update(new_profile);
        }

        Ok(())
    }

    pub async fn learn_from_action(
        &self,
        email_id: &str,
        action: &str,
        email: &Email,
    ) -> Result<Option<String>> {
        let prediction = self.predictions.get(email_id);
        let prompt = self.build_action_learning_prompt(action, prediction, email);
        let response = run_claude_prompt(&prompt, Duration::from_secs(60), false)
            .await
            .context("Claude CLI timed out after 60s for action learning")?;
        if response.contains("NO_UPDATE_NEEDED") {
            return Ok(None);
        }

        Ok(extract_profile_update(&response))
    }

    async fn get_batched_profile_update(
        &self,
        corrections: &[Correction],
    ) -> Result<Option<String>> {
        let prompt = self.build_batched_profile_prompt(corrections);
        let response = run_claude_prompt(&prompt, Duration::from_secs(90), true)
            .await
            .context("Claude CLI timed out after 90s for profile update")?;
        if response.contains("NO_UPDATE_NEEDED") {
            return Ok(None);
        }

        Ok(extract_profile_update(&response))
    }

    async fn load_current_email(
        &self,
        prediction: &Prediction,
        deleted_ids: &mut Vec<String>,
    ) -> Option<Email> {
        match self.provider.get_message(&prediction.email_id).await {
            Ok(email) => Some(email),
            Err(_) => {
                deleted_ids.push(prediction.email_id.clone());
                None
            }
        }
    }

    fn labels_changed(&self, prediction: &Prediction, email: &Email) -> bool {
        let predicted_labels = prediction.all_labels();
        let removed_labels = predicted_labels
            .iter()
            .filter(|label| label_was_removed(label, email))
            .count();
        if removed_labels > 0 {
            return true;
        }

        email.labels.iter().any(|label| {
            was_user_added_label(label, &predicted_labels, &prediction.pre_existing_labels)
        })
    }

    fn build_action_learning_prompt(
        &self,
        action: &str,
        prediction: Option<&Prediction>,
        email: &Email,
    ) -> String {
        let body_preview: String = email.body.chars().take(500).collect();
        let prediction_summary = prediction
            .map(format_prediction_summary)
            .unwrap_or_else(|| "No previous prediction".to_string());

        format!(
            r#"The user took an action on an email. Update the classification profile to learn from this.

Action: {}
{}

Email:
From: {}
Subject: {}
Body preview: {}

Current profile:
{}

If this action reveals a new pattern that should be added to the profile, output the COMPLETE updated profile.md.
If no update is needed (the profile already covers this case), respond with just: NO_UPDATE_NEEDED"#,
            action,
            prediction_summary,
            email.from,
            email.subject,
            body_preview,
            self.profile.content()
        )
    }

    fn build_batched_profile_prompt(&self, corrections: &[Correction]) -> String {
        let corrections_text = corrections
            .iter()
            .map(format_correction_block)
            .collect::<Vec<_>>()
            .join("\n\n");

        format!(
            r#"The user corrected these email classifications. Update the profile rules to prevent these mistakes.

Corrections:
{}

Current profile:
{}

Output the COMPLETE updated profile.md with new rules/patterns added.
If no meaningful patterns can be extracted, respond with just: NO_UPDATE_NEEDED"#,
            corrections_text,
            self.profile.content()
        )
    }
}

pub fn is_system_label(label: &str) -> bool {
    // Gmail system labels
    if matches!(
        label,
        "INBOX"
            | "SENT"
            | "DRAFT"
            | "TRASH"
            | "SPAM"
            | "STARRED"
            | "IMPORTANT"
            | "UNREAD"
            | "CATEGORY_PERSONAL"
            | "CATEGORY_SOCIAL"
            | "CATEGORY_PROMOTIONS"
            | "CATEGORY_UPDATES"
            | "CATEGORY_FORUMS"
    ) {
        return true;
    }
    // Internal labels we use for tracking
    label.eq_ignore_ascii_case("Classified")
}

/// Labels that are removed as part of normal workflow (not corrections).
/// Users remove these after taking action - this is expected behavior.
fn is_active_label(label: &str) -> bool {
    matches!(
        label.to_lowercase().as_str(),
        "needs-reply" | "important" | "urgent" | "awaiting-reply"
    )
}

fn label_was_removed(predicted_label: &str, email: &Email) -> bool {
    !is_active_label(predicted_label)
        && !email
            .labels
            .iter()
            .any(|email_label| email_label.eq_ignore_ascii_case(predicted_label))
}

fn was_user_added_label(
    email_label: &str,
    predicted_labels: &[String],
    pre_existing_labels: &[String],
) -> bool {
    if is_system_label(email_label) {
        return false;
    }

    let predicted_label_present = predicted_labels
        .iter()
        .any(|predicted_label| predicted_label.eq_ignore_ascii_case(email_label));
    if predicted_label_present {
        return false;
    }

    !pre_existing_labels
        .iter()
        .any(|pre_existing_label| pre_existing_label.eq_ignore_ascii_case(email_label))
}

fn build_correction(prediction: &Prediction, email: &Email, actual_spam: bool) -> Correction {
    Correction {
        email_id: prediction.email_id.clone(),
        from: prediction.from.clone(),
        subject: prediction.subject.clone(),
        predicted_labels: sorted_labels(prediction.all_labels()),
        actual_labels: sorted_labels(non_system_labels(email)),
        predicted_spam: prediction.is_spam,
        actual_spam,
    }
}

fn sorted_labels(mut labels: Vec<String>) -> Vec<String> {
    labels.sort();
    labels
}

fn non_system_labels(email: &Email) -> Vec<String> {
    email
        .labels
        .iter()
        .filter(|label| !is_system_label(label))
        .cloned()
        .collect()
}

fn describe_correction(date: &str, correction: &Correction) -> String {
    if correction.predicted_spam != correction.actual_spam {
        return describe_spam_correction(date, correction);
    }

    format!(
        "{}: User relabeled email (from: {}, predicted: {:?}, actual: {:?})",
        date, correction.from, correction.predicted_labels, correction.actual_labels
    )
}

fn describe_spam_correction(date: &str, correction: &Correction) -> String {
    if correction.actual_spam {
        return format!(
            "{}: User marked email as spam (from: {}, subject: {})",
            date, correction.from, correction.subject
        );
    }

    format!(
        "{}: User unmarked spam (false positive, from: {}, subject: {})",
        date, correction.from, correction.subject
    )
}

fn format_prediction_summary(prediction: &Prediction) -> String {
    format!(
        "Previous prediction: is_spam={}, labels={:?}",
        prediction.is_spam,
        prediction.all_labels()
    )
}

fn format_correction_block(correction: &Correction) -> String {
    format!(
        "- From: {}\n  Subject: {}\n  Predicted: {:?}\n  Actual: {:?}",
        correction.from, correction.subject, correction.predicted_labels, correction.actual_labels
    )
}

async fn run_claude_prompt(
    prompt: &str,
    timeout_duration: Duration,
    use_stdin: bool,
) -> Result<String> {
    let response = if use_stdin {
        run_claude_prompt_via_stdin(prompt, timeout_duration).await?
    } else {
        run_claude_prompt_via_args(prompt, timeout_duration).await?
    };

    Ok(response.trim().to_string())
}

async fn run_claude_prompt_via_args(prompt: &str, timeout_duration: Duration) -> Result<String> {
    let output = timeout(
        timeout_duration,
        Command::new("claude")
            .args([
                "-p",
                prompt,
                "--model",
                CLAUDE_MODEL,
                "--disallowedTools",
                CLAUDE_BROWSER_TOOLS,
                "--no-session-persistence",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("Failed to run claude CLI")??;

    parse_claude_output(output, true)
}

async fn run_claude_prompt_via_stdin(prompt: &str, timeout_duration: Duration) -> Result<String> {
    let prompt_file = std::env::temp_dir().join("email-assistant-profile-prompt.txt");
    let _ = std::fs::write(&prompt_file, prompt);

    let mut child = Command::new("claude")
        .args([
            "-p",
            "-",
            "--model",
            CLAUDE_MODEL,
            "--disallowedTools",
            CLAUDE_BROWSER_TOOLS,
            "--no-session-persistence",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn claude CLI")?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(prompt.as_bytes()).await?;
    }

    let output = timeout(timeout_duration, child.wait_with_output())
        .await
        .context("Failed to run claude CLI")??;

    parse_claude_output(output, false)
}

fn parse_claude_output(output: std::process::Output, require_success: bool) -> Result<String> {
    if !output.status.success() {
        if require_success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude CLI failed: {}", stderr);
        }
        return Ok(String::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_profile_update(response: &str) -> Option<String> {
    // Look for markdown code block
    if let Some(start) = response.find("```markdown") {
        if let Some(end) = response[start + 11..].find("```") {
            return Some(response[start + 11..start + 11 + end].trim().to_string());
        }
    }

    // Look for generic code block
    if let Some(start) = response.find("```") {
        let content_start = start + 3;
        // Skip language identifier if present
        let actual_start = response[content_start..]
            .find('\n')
            .map(|i| content_start + i + 1)
            .unwrap_or(content_start);
        if let Some(end) = response[actual_start..].find("```") {
            return Some(
                response[actual_start..actual_start + end]
                    .trim()
                    .to_string(),
            );
        }
    }

    // If response looks like a profile (starts with #), use it directly
    let trimmed = response.trim();
    if trimmed.starts_with("# Email Classification Profile") {
        return Some(trimmed.to_string());
    }

    None
}
