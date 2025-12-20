use crate::predictions::PredictionStore;
use crate::profile::Profile;
use crate::providers::{Email, EmailProvider};
use anyhow::{Context, Result};
use chrono::Utc;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

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

pub struct LearningEngine<'a, P: EmailProvider> {
    provider: &'a P,
    profile: &'a mut Profile,
    predictions: &'a PredictionStore,
}

impl<'a, P: EmailProvider> LearningEngine<'a, P> {
    pub fn new(provider: &'a P, profile: &'a mut Profile, predictions: &'a PredictionStore) -> Self {
        Self {
            provider,
            profile,
            predictions,
        }
    }

    pub async fn detect_corrections(&self) -> Result<LearningResult> {
        let mut result = LearningResult::default();

        for prediction in self.predictions.all_predictions() {
            // Fetch current state of the email
            let email = match self.provider.get_message(&prediction.email_id).await {
                Ok(e) => e,
                Err(_) => {
                    // Email deleted - just clean up prediction, don't learn
                    result.deleted_ids.push(prediction.email_id.clone());
                    continue;
                }
            };

            let actual_spam = email.labels.iter().any(|l| l == "SPAM");
            let spam_mismatch = prediction.is_spam != actual_spam;

            // Check if our predicted labels are still on the email (case-insensitive)
            let predicted_labels = prediction.all_labels();
            let removed_labels: Vec<_> = predicted_labels.iter()
                .filter(|pred_label| {
                    !email.labels.iter().any(|email_label|
                        email_label.eq_ignore_ascii_case(pred_label)
                    )
                })
                .cloned()
                .collect();

            // Check for new user-added labels (excluding system labels, case-insensitive)
            let added_labels: Vec<_> = email.labels.iter()
                .filter(|email_label| {
                    !is_system_label(email_label) &&
                    !predicted_labels.iter().any(|pred_label|
                        pred_label.eq_ignore_ascii_case(email_label)
                    )
                })
                .cloned()
                .collect();

            let label_mismatch = !removed_labels.is_empty() || !added_labels.is_empty();

            if spam_mismatch || label_mismatch {
                result.corrections.push(Correction {
                    email_id: prediction.email_id.clone(),
                    from: prediction.from.clone(),
                    subject: prediction.subject.clone(),
                    predicted_labels: prediction.all_labels(),
                    actual_labels: email.labels.iter()
                        .filter(|l| !is_system_label(l))
                        .cloned()
                        .collect(),
                    predicted_spam: prediction.is_spam,
                    actual_spam,
                });
            }
        }

        Ok(result)
    }

    pub async fn apply_corrections(&mut self, corrections: &[Correction]) -> Result<()> {
        if corrections.is_empty() {
            return Ok(());
        }

        let date = Utc::now().format("%Y-%m-%d").to_string();

        // Record all corrections in the profile
        for correction in corrections {
            let description = if correction.predicted_spam != correction.actual_spam {
                if correction.actual_spam {
                    format!(
                        "{}: User marked email as spam (from: {}, subject: {})",
                        date, correction.from, correction.subject
                    )
                } else {
                    format!(
                        "{}: User unmarked spam (false positive, from: {}, subject: {})",
                        date, correction.from, correction.subject
                    )
                }
            } else {
                format!(
                    "{}: User relabeled email (from: {}, predicted: {:?}, actual: {:?})",
                    date, correction.from, correction.predicted_labels, correction.actual_labels
                )
            };
            self.profile.append_correction(&description);
        }

        // Batch all corrections into a single Claude call
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
        // Check if we had a prediction for this email
        let prediction = self.predictions.get(email_id);

        let prompt = format!(
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
            if let Some(p) = prediction {
                format!("Previous prediction: is_spam={}, labels={:?}", p.is_spam, p.labels)
            } else {
                "No previous prediction".to_string()
            },
            email.from,
            email.subject,
            email.body.chars().take(500).collect::<String>(),
            self.profile.content()
        );

        let output = timeout(
            Duration::from_secs(60),
            Command::new("claude")
                .args(["-p", &prompt, "--model", "haiku", "--tools", "", "--mcp-config", "", "--no-session-persistence"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        )
        .await
        .context("Claude CLI timed out after 60s for action learning")?
        .context("Failed to run claude CLI")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude CLI failed: {}", stderr);
        }

        let response = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if response.contains("NO_UPDATE_NEEDED") {
            return Ok(None);
        }

        // Extract the profile update (diff or full)
        let update = extract_profile_update(&response);
        Ok(update)
    }

    async fn get_batched_profile_update(
        &self,
        corrections: &[Correction],
    ) -> Result<Option<String>> {
        let corrections_text: Vec<String> = corrections.iter().map(|c| {
            format!(
                "- From: {}\n  Subject: {}\n  Predicted: {:?}\n  Actual: {:?}",
                c.from, c.subject, c.predicted_labels, c.actual_labels
            )
        }).collect();

        let prompt = format!(
            r#"The user corrected these email classifications. Update the profile rules to prevent these mistakes.

Corrections:
{}

Current profile:
{}

Output the COMPLETE updated profile.md with new rules/patterns added.
If no meaningful patterns can be extracted, respond with just: NO_UPDATE_NEEDED"#,
            corrections_text.join("\n\n"),
            self.profile.content()
        );

        // Save prompt to file for debugging
        let prompt_file = std::env::temp_dir().join("email-assistant-profile-prompt.txt");
        let _ = std::fs::write(&prompt_file, &prompt);

        // Use stdin for prompt to avoid CLI arg length limits
        let mut child = Command::new("claude")
            .args(["-p", "-", "--model", "haiku", "--tools", "", "--mcp-config", "", "--no-session-persistence"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn claude CLI")?;

        // Write prompt to stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(prompt.as_bytes()).await?;
        }

        let output = timeout(Duration::from_secs(90), child.wait_with_output())
            .await
            .context("Claude CLI timed out after 90s for profile update")?
            .context("Failed to run claude CLI")?;

        if !output.status.success() {
            return Ok(None);
        }

        let response = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if response.contains("NO_UPDATE_NEEDED") {
            return Ok(None);
        }

        Ok(extract_profile_update(&response))
    }
}

fn is_system_label(label: &str) -> bool {
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
            return Some(response[actual_start..actual_start + end].trim().to_string());
        }
    }

    // If response looks like a profile (starts with #), use it directly
    let trimmed = response.trim();
    if trimmed.starts_with("# Email Classification Profile") {
        return Some(trimmed.to_string());
    }

    None
}
