use crate::learning::LearningEngine;
use crate::predictions::{Prediction, PredictionStore};
use crate::profile::Profile;
use crate::providers::{Email, EmailProvider};
use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

const CLAUDE_MODEL: &str = "haiku";
const CLAUDE_BROWSER_TOOLS: &str = "mcp__browsermcp__browser_navigate,mcp__browsermcp__browser_click,mcp__browsermcp__browser_snapshot,mcp__browsermcp__browser_screenshot,mcp__browsermcp__browser_wait,mcp__browsermcp__browser_hover,mcp__browsermcp__browser_type,mcp__browsermcp__browser_select_option,mcp__browsermcp__browser_press_key,mcp__browsermcp__browser_go_back,mcp__browsermcp__browser_go_forward,mcp__browsermcp__browser_get_console_logs";

pub fn print_action_preview(action: &str, email: &Email) {
    println!("Would {}: \"{}\"", action, email.subject);
    println!("  From: {}", email.from);
}

pub async fn learn_from_manual_action(
    provider: &dyn EmailProvider,
    profile: &mut Profile,
    predictions: &PredictionStore,
    id: &str,
    action: &str,
    email: &Email,
) -> Result<()> {
    let learning = LearningEngine::new(provider, profile, predictions);
    if let Some(update) = learning.learn_from_action(id, action, email).await? {
        println!("\n📝 Profile updated:");
        println!("{}", update);
        profile.save()?;
    }
    Ok(())
}

pub fn print_needs_reply_entry(prediction: &Prediction, email: &Email) {
    let is_unread = email.labels.iter().any(|label| label == "UNREAD");
    let marker = if is_unread { "●" } else { " " };

    println!(
        "{} {} | {} | {:?}",
        marker,
        prediction.email_id,
        prediction.subject.chars().take(50).collect::<String>(),
        prediction.all_labels()
    );
}

pub fn summary_prompt(emails: &[Email]) -> String {
    let email_text = format_summary_emails(emails);

    format!(
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
    )
}

fn format_summary_emails(emails: &[Email]) -> String {
    let mut email_text = String::new();

    for (index, email) in emails.iter().enumerate() {
        let body_preview: String = email.body.chars().take(2000).collect();
        email_text.push_str(&format!(
            "=== Email {} ===\nFrom: {}\nSubject: {}\nBody:\n{}\n\n",
            index + 1,
            email.from,
            email.subject,
            body_preview
        ));
    }

    email_text
}

pub async fn run_summary_prompt(prompt: &str) -> Result<String> {
    let prompt_file = std::env::temp_dir().join("email-assistant-summary-prompt.txt");
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

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn build_status_indicators(labels: &[String], is_important: bool) -> String {
    let in_inbox = labels.iter().any(|label| label == "INBOX");
    let is_starred = labels.iter().any(|label| label == "STARRED");
    let is_unread = labels.iter().any(|label| label == "UNREAD");

    let unread_marker = if is_unread { "●" } else { " " };
    let location_marker = if is_starred {
        "*"
    } else if !in_inbox {
        "A"
    } else {
        " "
    };
    let priority_marker = if is_important { "🔥" } else { " " };

    format!("[{}{}{}]", unread_marker, location_marker, priority_marker)
}
