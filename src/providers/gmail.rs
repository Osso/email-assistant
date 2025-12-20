use super::{Email, EmailProvider, Label};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct GmailProvider {
    client: gmail::Client,
    label_id_to_name: HashMap<String, String>,
}

impl GmailProvider {
    pub async fn new() -> Result<Self> {
        let cfg = gmail::config::load_config()?;
        let client_id = cfg.client_id.ok_or_else(|| {
            anyhow::anyhow!("Gmail not configured. Run 'gmail config <client-id>' first")
        })?;
        let client_secret = cfg.client_secret.ok_or_else(|| {
            anyhow::anyhow!("Gmail not configured. Run 'gmail config <client-id>' first")
        })?;

        let tokens = gmail::config::load_tokens()
            .context("Not logged in. Run 'gmail login' first")?;

        // Try to use existing token, refresh if needed
        let client = gmail::Client::new(&tokens.access_token);

        // Test if token works
        let client = match client.list_messages(None, "INBOX", 1).await {
            Ok(_) => client,
            Err(_) => {
                // Token expired, try refresh
                let new_tokens =
                    gmail::auth::refresh_token(&client_id, &client_secret, &tokens.refresh_token)
                        .await?;
                gmail::Client::new(&new_tokens.access_token)
            }
        };

        // Build label ID to name mapping
        let mut label_id_to_name = HashMap::new();
        if let Ok(labels) = client.list_labels().await {
            if let Some(label_list) = labels.labels {
                for label in label_list {
                    label_id_to_name.insert(label.id, label.name);
                }
            }
        }

        Ok(Self { client, label_id_to_name })
    }

    fn resolve_label_ids(&self, label_ids: Vec<String>) -> Vec<String> {
        label_ids.into_iter()
            .map(|id| {
                self.label_id_to_name.get(&id)
                    .cloned()
                    .unwrap_or(id)
            })
            .collect()
    }

    fn message_to_email(&self, msg: gmail::Message) -> Email {
        let label_ids = msg.label_ids.clone().unwrap_or_default();
        // Use body text if available, fall back to snippet
        let body = msg.get_body_text()
            .or_else(|| msg.snippet.clone())
            .unwrap_or_default();
        Email {
            id: msg.id.clone(),
            from: msg.get_header("From").unwrap_or("").to_string(),
            to: msg.get_header("To").unwrap_or("").to_string(),
            subject: msg.get_header("Subject").unwrap_or("(no subject)").to_string(),
            body,
            labels: self.resolve_label_ids(label_ids),
        }
    }
}

#[async_trait]
impl EmailProvider for GmailProvider {
    async fn list_messages(&self, max: u32, label: &str, query: Option<&str>) -> Result<Vec<Email>> {
        let list = self.client.list_messages(query, label, max).await?;

        let mut emails = Vec::new();
        if let Some(messages) = list.messages {
            for msg_ref in messages {
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
        let list = self.client.list_labels().await?;
        let mut labels = Vec::new();

        if let Some(gmail_labels) = list.labels {
            for label in gmail_labels {
                labels.push(Label {
                    id: label.id,
                    name: label.name,
                });
            }
        }

        Ok(labels)
    }

    async fn add_label(&self, id: &str, label: &str) -> Result<()> {
        self.client.add_label(id, label).await
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
