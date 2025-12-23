use super::{Email, EmailProvider, Label};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct OutlookProvider {
    client: outlook::api::Client,
    #[allow(dead_code)] // May be used for category name lookup later
    category_id_to_name: HashMap<String, String>,
}

impl OutlookProvider {
    pub async fn new() -> Result<Self> {
        let cfg = outlook::config::load_config()?;
        let client_id = cfg.client_id();

        let tokens = outlook::config::load_tokens()
            .context("Not logged in. Run 'outlook login' first")?;

        // Try to use existing token, refresh if needed
        let client = outlook::api::Client::new(&tokens.access_token);

        // Test if token works by listing one message
        let client = match client.list_messages("inbox", None, 1).await {
            Ok(_) => client,
            Err(_) => {
                // Token expired, try refresh
                let new_tokens =
                    outlook::auth::refresh_token(client_id, &tokens.refresh_token)
                        .await?;
                outlook::api::Client::new(&new_tokens.access_token)
            }
        };

        // Build category ID to name mapping
        let mut category_id_to_name = HashMap::new();
        if let Ok(categories) = client.list_categories().await {
            if let Some(cat_list) = categories.value {
                for cat in cat_list {
                    if let Some(id) = cat.id {
                        category_id_to_name.insert(id, cat.display_name);
                    }
                }
            }
        }

        Ok(Self { client, category_id_to_name })
    }

    fn resolve_category_ids(&self, category_names: Vec<String>) -> Vec<String> {
        // Outlook categories are already names, not IDs like Gmail
        // But we keep this for consistency
        category_names
    }

    fn message_to_email(&self, msg: outlook::api::Message) -> Email {
        let categories = msg.categories.clone().unwrap_or_default();

        // Build pseudo-labels from Outlook state
        let mut labels = self.resolve_category_ids(categories);

        // Add INBOX pseudo-label (outlook list defaults to inbox)
        labels.push("INBOX".to_string());

        // Add UNREAD pseudo-label if not read
        if msg.is_read == Some(false) {
            labels.push("UNREAD".to_string());
        }

        // Use body text if available, fall back to body preview
        let body = msg.get_body_text()
            .or_else(|| msg.body_preview.clone())
            .map(|b| strip_html(&b))
            .unwrap_or_default();

        Email {
            id: msg.id.clone(),
            from: msg.get_from().unwrap_or_default(),
            to: msg.get_to().unwrap_or_default(),
            subject: msg.subject.clone().unwrap_or_else(|| "(no subject)".to_string()),
            body,
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
        // Map Gmail-style label to Outlook folder
        let folder = match label {
            "INBOX" => "inbox",
            "SENT" => "sentitems",
            "TRASH" => "deleteditems",
            "SPAM" => "junkemail",
            _ => "inbox",
        };

        // Handle query - convert -label:X to category filter
        let filter = query.and_then(|q| {
            if q.starts_with("-label:") {
                let cat = q.trim_start_matches("-label:");
                // OData filter to exclude category
                Some(format!("NOT (categories/any(c:c eq '{}'))", cat))
            } else {
                None
            }
        });

        let list = self.client.list_messages(folder, filter.as_deref(), max).await?;

        let mut emails = Vec::new();
        if let Some(messages) = list.value {
            for msg_ref in messages {
                // Get full message with body
                let msg = self.client.get_message(&msg_ref.id).await?;
                emails.push(self.message_to_email(msg));
            }
        }

        Ok(emails)
    }

    async fn get_message(&self, id: &str) -> Result<Email> {
        let msg = self.client.get_message(id).await?;
        Ok(self.message_to_email(msg))
    }

    async fn list_labels(&self) -> Result<Vec<Label>> {
        let list = self.client.list_categories().await?;
        let mut labels = Vec::new();

        if let Some(categories) = list.value {
            for cat in categories {
                labels.push(Label {
                    id: cat.id.unwrap_or_else(|| cat.display_name.clone()),
                    name: cat.display_name,
                });
            }
        }

        Ok(labels)
    }

    async fn add_label(&self, id: &str, label: &str) -> Result<()> {
        // Ensure category exists in master list first
        self.client.ensure_category(label).await?;
        self.client.add_category(id, label).await
    }

    async fn mark_spam(&self, id: &str) -> Result<()> {
        self.client.mark_spam(id).await
    }

    async fn unspam(&self, id: &str) -> Result<()> {
        self.client.unspam(id).await
    }

    async fn archive(&self, id: &str) -> Result<()> {
        self.client.archive(id).await
    }

    async fn trash(&self, id: &str) -> Result<()> {
        self.client.trash(id).await
    }
}
