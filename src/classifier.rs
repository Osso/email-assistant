use crate::profile::Profile;
use crate::providers::Email;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const CLASSIFICATION_PROMPT: &str = r#"You are an email classifier. Analyze this email and assign appropriate labels.

<profile>
__PROFILE__
</profile>

<email>
From: __FROM__
To: __TO__
Subject: __SUBJECT__
Body: __BODY__
</email>

Classify this email:
- is_spam: true if malicious/scam/phishing/horoscope/astrology/psychic spam, false for legitimate newsletters
- theme: 1-5 labels describing what email is about. Examples: "Receipts" (payment confirmations AFTER charge), "Bills" (upcoming payments, auto-renewal notices, subscription charges - archive if auto-pay), "Finance", "Health", "Shopping", "Travel", "Work", "Personal", "Social", "Security", "Gaming", "Shipping", "Updates", "Account", "Home" (smart home alerts, leak sensors, thermostat, security cameras)
- action: 0+ labels for what to do. Options:
  - "Newsletters" - regular subscription content you signed up for
  - "Promotional" - ads, sales, marketing, webinar invites from companies (auto-delete)
  - "Survey" - feedback requests, satisfaction surveys, NPS scores (auto-archive)
  - "Needs-Reply" - expects a response from you (questions, requests, invitations). Archive unless reply needed today/tomorrow
  - "Important" - requires your attention today
  - "Urgent" - time-sensitive, needs immediate attention (security alerts are always Urgent)
  - "Awaiting-Reply" - you sent something and are waiting for response, no action needed now (auto-archive)
  - "Group-Thread" - group thread/discussion where you're CC'd (auto-archive)
  - "Other" - doesn't fit other categories (auto-archive)
- archive: true if email doesn't need to stay in inbox (Newsletters, Survey, Awaiting-Reply, Group-Thread, Other, Updates, Needs-Reply without urgency, Bills without Needs-Reply, receipts under $500, account notifications without action required). NEVER archive Security emails
- delete: true if is_spam OR Promotional OR expired calendar invites (date in the past) OR matches auto-delete rules in profile (including language rules). CHECK THE TO FIELD - if email is TO a work address listed in Auto-Delete Rules, set delete=true. NEVER delete Personal emails, Needs-Reply emails, or emails from personal contacts. "Personal" means from someone you know, NOT spam with your name in it

Respond with JSON only:
{{"is_spam": false, "theme": ["Finance"], "action": ["Important"], "archive": false, "delete": false, "confidence": 0.8}}"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub is_spam: bool,
    #[serde(default)]
    pub archive: bool,
    #[serde(default)]
    pub delete: bool,
    /// Theme labels (1-5): what the email is about
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
        self.theme
            .iter()
            .chain(self.action.iter())
            .cloned()
            .collect()
    }
}

/// Response event from `claude --output-format json`
#[derive(Debug, Deserialize)]
struct ClaudeEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    result: Option<String>,
}

pub struct Classifier<'a> {
    profile: &'a Profile,
}

impl<'a> Classifier<'a> {
    pub fn new(profile: &'a Profile) -> Self {
        Self { profile }
    }

    pub async fn classify(&self, email: &Email) -> Result<Classification> {
        let prompt = self.build_prompt(email);
        let output = claude_safe::call(&prompt, "opus", "json")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call claude: {}", e))?;
        let result_text = parse_result_text(&output)?;
        let json_str = extract_json(&result_text)?;
        let mut classification: Classification =
            serde_json::from_str(&json_str).context("Failed to parse classification response")?;
        normalize_classification(&mut classification);
        Ok(classification)
    }

    fn build_prompt(&self, email: &Email) -> String {
        let body_preview: String = email.body.chars().take(1000).collect();

        CLASSIFICATION_PROMPT
            .replace("__PROFILE__", self.profile.content())
            .replace("__FROM__", &email.from)
            .replace("__TO__", &email.to)
            .replace("__SUBJECT__", &email.subject)
            .replace("__BODY__", &body_preview)
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

fn parse_result_text(output: &str) -> Result<String> {
    let events: Vec<ClaudeEvent> =
        serde_json::from_str(output).context("Failed to parse claude response events")?;

    events
        .iter()
        .rev()
        .find(|e| e.event_type == "result")
        .and_then(|e| e.result.clone())
        .context("No result found in claude response")
}

fn normalize_classification(classification: &mut Classification) {
    classification.theme = normalize_labels(std::mem::take(&mut classification.theme));
    classification.action = normalize_labels(std::mem::take(&mut classification.action));
}

fn normalize_labels(labels: Vec<String>) -> Vec<String> {
    labels
        .into_iter()
        .filter(|label| !label.eq_ignore_ascii_case("Classified"))
        .map(|label| capitalize_first(&label))
        .collect()
}

fn extract_json(text: &str) -> Result<String> {
    let text = text.trim();

    if let Some(json) = extract_leading_json_object(text) {
        return Ok(json);
    }
    if let Some(json) = extract_fenced_json(text) {
        return Ok(json);
    }
    if let Some(json) = extract_braced_json(text) {
        return Ok(json);
    }
    anyhow::bail!("Could not find JSON in response: {}", text)
}

fn extract_leading_json_object(text: &str) -> Option<String> {
    let end = json_object_end(text)?;
    Some(text[..end].to_string())
}

fn extract_fenced_json(text: &str) -> Option<String> {
    let start = text.find("```json")?;
    let fenced = &text[start + 7..];
    let end = fenced.find("```")?;
    Some(fenced[..end].trim().to_string())
}

fn extract_braced_json(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}

fn json_object_end(text: &str) -> Option<usize> {
    if !text.starts_with('{') {
        return None;
    }

    let mut depth = 0;
    text.char_indices().find_map(|(index, ch)| {
        depth = next_json_depth(depth, ch)?;
        (depth == 0).then_some(index + ch.len_utf8())
    })
}

fn next_json_depth(depth: usize, ch: char) -> Option<usize> {
    match ch {
        '{' => Some(depth + 1),
        '}' => depth.checked_sub(1),
        _ => Some(depth),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let json =
            r#"{"is_spam": false, "is_important": true, "labels": ["work"], "confidence": 0.9}"#;
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
