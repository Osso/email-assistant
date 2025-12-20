use crate::profile::Profile;
use crate::providers::Email;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub is_spam: bool,
    #[serde(default)]
    pub archive: bool,
    #[serde(default)]
    pub delete: bool,
    /// Theme labels (1-2): what the email is about
    #[serde(default)]
    pub theme: Vec<String>,
    /// Action labels: what to do with it
    #[serde(default)]
    pub action: Vec<String>,
    pub confidence: f32,
}

impl Classification {
    /// Combined labels for backward compatibility
    pub fn labels(&self) -> Vec<String> {
        self.theme.iter().chain(self.action.iter()).cloned().collect()
    }
}

/// Response wrapper from `claude --output-format json`
#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    result: String,
}

pub struct Classifier<'a> {
    profile: &'a Profile,
}

impl<'a> Classifier<'a> {
    pub fn new(profile: &'a Profile) -> Self {
        Self { profile }
    }

    pub async fn classify(&self, email: &Email) -> Result<Classification> {
        let body_preview: String = email.body.chars().take(1000).collect();

        let prompt = format!(
            r#"You are an email classifier. Analyze this email and assign appropriate labels.

<profile>
{}
</profile>

<email>
From: {}
Subject: {}
Body: {}
</email>

Classify this email:
- is_spam: true ONLY if clearly malicious/scam/phishing, false for newsletters and promotions
- theme: 1-2 labels describing what email is about. Examples: "Receipts", "Bills", "Finance", "Health", "Shopping", "Travel", "Work", "Personal", "Social", "Security", "Gaming", "Shipping", "Updates"
- action: 0+ labels for what to do. Options:
  - "Newsletters" - regular subscription content you signed up for
  - "Promotional" - ads, sales, marketing from companies (auto-archive)
  - "Needs-Reply" - expects a response from you (questions, requests, invitations). Archive unless reply needed today/tomorrow
  - "Important" - requires your attention today
  - "Urgent" - time-sensitive, needs immediate attention (security alerts are always Urgent)
  - "Awaiting-Reply" - you sent something and are waiting for response, no action needed now (auto-archive)
  - "FYI" - group thread/discussion, you're CC'd or just informed (auto-archive)
  - "Other" - doesn't fit other categories (auto-archive)
- archive: true if email doesn't need to stay in inbox (Newsletters, Promotional, Awaiting-Reply, FYI, Other, Updates, Needs-Reply without urgency, Bills without Needs-Reply, receipts under $500). NEVER archive Security emails
- delete: true if email matches auto-delete rules in profile (check Auto-Delete Rules section)

Respond with JSON only:
{{"is_spam": false, "theme": ["Finance"], "action": ["Important"], "archive": false, "delete": false, "confidence": 0.8}}"#,
            self.profile.content(),
            email.from,
            email.subject,
            body_preview
        );

        let output = Command::new("claude")
            .args(["-p", &prompt, "--output-format", "json", "--model", "haiku"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run claude CLI")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude CLI failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse the wrapper JSON from claude --output-format json
        let wrapper: ClaudeResponse = serde_json::from_str(&stdout)
            .context("Failed to parse claude response wrapper")?;

        // Extract the classification JSON from the result text
        let json_str = extract_json(&wrapper.result)?;

        let mut classification: Classification =
            serde_json::from_str(&json_str).context("Failed to parse classification response")?;

        // Filter out internal labels that shouldn't be suggested by LLM
        classification.theme.retain(|l| !l.eq_ignore_ascii_case("Classified"));
        classification.action.retain(|l| !l.eq_ignore_ascii_case("Classified"));

        // Capitalize first letter of each label for consistency
        classification.theme = classification.theme
            .into_iter()
            .map(|l| capitalize_first(&l))
            .collect();
        classification.action = classification.action
            .into_iter()
            .map(|l| capitalize_first(&l))
            .collect();

        Ok(classification)
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

fn extract_json(text: &str) -> Result<String> {
    // Try to find JSON object in the text
    let text = text.trim();

    // If it starts with {, assume it's JSON
    if text.starts_with('{') {
        // Find the matching closing brace
        let mut depth = 0;
        let mut end = 0;
        for (i, c) in text.chars().enumerate() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if end > 0 {
            return Ok(text[..end].to_string());
        }
    }

    // Try to find JSON in code blocks
    if let Some(start) = text.find("```json") {
        if let Some(end) = text[start..].find("```\n").or(text[start..].rfind("```")) {
            let json_start = start + 7; // Skip ```json
            return Ok(text[json_start..start + end].trim().to_string());
        }
    }

    // Last resort: find first { and last }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if end > start {
            return Ok(text[start..=end].to_string());
        }
    }

    anyhow::bail!("Could not find JSON in response: {}", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let json = r#"{"is_spam": false, "is_important": true, "labels": ["work"], "confidence": 0.9}"#;
        assert_eq!(extract_json(json).unwrap(), json);
    }

    #[test]
    fn test_extract_json_with_whitespace() {
        let text = r#"
        {"is_spam": false, "is_important": true, "labels": [], "confidence": 0.8}
        "#;
        assert!(extract_json(text).unwrap().contains("is_spam"));
    }
}
