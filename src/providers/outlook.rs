use super::{Email, EmailProvider, Label};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutlookMessage {
    id: String,
    from: String,
    #[serde(default)]
    to: String,
    subject: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    snippet: String,
    date: String,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    is_read: bool,
}

#[derive(Debug, Deserialize)]
struct OutlookCategory {
    #[serde(rename = "displayName")]
    display_name: String,
    id: String,
}

pub struct OutlookProvider;

impl OutlookProvider {
    pub async fn new() -> Result<Self> {
        // Verify outlook CLI is available and authenticated
        let output = Command::new("outlook")
            .args(["list", "-n", "1", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook CLI. Is it installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Not logged in") || stderr.contains("token") {
                anyhow::bail!("Outlook not authenticated. Run 'outlook login' first");
            }
            anyhow::bail!("Outlook CLI error: {}", stderr);
        }

        Ok(Self)
    }

    fn message_to_email(msg: OutlookMessage) -> Email {
        // Build pseudo-labels from Outlook state
        let mut labels = msg.categories.clone();

        // Add INBOX pseudo-label (outlook list defaults to inbox)
        labels.push("INBOX".to_string());

        // Add UNREAD pseudo-label if not read
        if !msg.is_read {
            labels.push("UNREAD".to_string());
        }

        // Use snippet as body if full body not available, strip HTML
        let body = if msg.body.is_empty() {
            msg.snippet.clone()
        } else {
            strip_html(&msg.body)
        };

        Email {
            id: msg.id,
            from: msg.from,
            to: msg.to,
            subject: msg.subject,
            body,
            date: msg.date,
            labels,
        }
    }
}

fn strip_html(html: &str) -> String {
    // Simple HTML stripping - remove tags and decode common entities
    let mut result = String::new();
    let mut in_tag = false;

    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }

    // Decode common HTML entities
    result
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("\r\n", "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl EmailProvider for OutlookProvider {
    async fn list_messages(&self, max: u32, label: &str, query: Option<&str>) -> Result<Vec<Email>> {
        let max_str = max.to_string();
        let mut args = vec!["list", "-n", &max_str, "--json"];

        // Map Gmail-style label to Outlook folder
        let folder = match label {
            "INBOX" => "inbox",
            "SENT" => "sent",
            "TRASH" => "trash",
            "SPAM" => "spam",
            _ => "inbox",
        };
        args.extend(["-l", folder]);

        // Handle query - Outlook uses different query syntax
        // For now, we filter client-side for -label:X queries
        let exclude_category = query.and_then(|q| {
            if q.starts_with("-label:") {
                Some(q.trim_start_matches("-label:"))
            } else {
                None
            }
        });

        let output = Command::new("outlook")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook list")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook list failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let messages: Vec<OutlookMessage> = serde_json::from_str(&stdout)
            .context("Failed to parse outlook list output")?;

        let mut emails: Vec<Email> = messages
            .into_iter()
            .map(Self::message_to_email)
            .collect();

        // Client-side filtering for excluded categories
        if let Some(exclude) = exclude_category {
            emails.retain(|e| !e.labels.iter().any(|l| l.eq_ignore_ascii_case(exclude)));
        }

        Ok(emails)
    }

    async fn get_message(&self, id: &str) -> Result<Email> {
        let output = Command::new("outlook")
            .args(["read", id, "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook read")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook read failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg: OutlookMessage = serde_json::from_str(&stdout)
            .context("Failed to parse outlook read output")?;

        Ok(Self::message_to_email(msg))
    }

    async fn list_labels(&self) -> Result<Vec<Label>> {
        let output = Command::new("outlook")
            .args(["labels", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook labels")?;

        if !output.status.success() {
            // Categories API might not be available, return empty
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let categories: Vec<OutlookCategory> = serde_json::from_str(&stdout)
            .unwrap_or_default();

        Ok(categories
            .into_iter()
            .map(|c| Label {
                id: c.id,
                name: c.display_name,
            })
            .collect())
    }

    async fn add_label(&self, id: &str, label: &str) -> Result<()> {
        let output = Command::new("outlook")
            .args(["label", id, label])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook label")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook label failed: {}", stderr);
        }

        Ok(())
    }

    async fn remove_label(&self, id: &str, label: &str) -> Result<()> {
        let output = Command::new("outlook")
            .args(["unlabel", id, label])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook unlabel")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook unlabel failed: {}", stderr);
        }

        Ok(())
    }

    async fn mark_spam(&self, id: &str) -> Result<()> {
        let output = Command::new("outlook")
            .args(["spam", id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook spam")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook spam failed: {}", stderr);
        }

        Ok(())
    }

    async fn unspam(&self, id: &str) -> Result<()> {
        let output = Command::new("outlook")
            .args(["unspam", id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook unspam")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook unspam failed: {}", stderr);
        }

        Ok(())
    }

    async fn archive(&self, id: &str) -> Result<()> {
        let output = Command::new("outlook")
            .args(["archive", id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook archive")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook archive failed: {}", stderr);
        }

        Ok(())
    }

    async fn trash(&self, id: &str) -> Result<()> {
        let output = Command::new("outlook")
            .args(["delete", id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run outlook delete")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("outlook delete failed: {}", stderr);
        }

        Ok(())
    }
}
