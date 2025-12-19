use super::{Email, EmailProvider, Label};
use anyhow::{Context, Result};
use async_trait::async_trait;

pub struct GmailProvider {
    client: gmail::Client,
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
        match client.list_messages(None, "INBOX", 1).await {
            Ok(_) => Ok(Self { client }),
            Err(_) => {
                // Token expired, try refresh
                let new_tokens =
                    gmail::auth::refresh_token(&client_id, &client_secret, &tokens.refresh_token)
                        .await?;
                Ok(Self {
                    client: gmail::Client::new(&new_tokens.access_token),
                })
            }
        }
    }
}

#[async_trait]
impl EmailProvider for GmailProvider {
    async fn list_messages(&self, max: u32, label: &str) -> Result<Vec<Email>> {
        let list = self.client.list_messages(None, label, max).await?;

        let mut emails = Vec::new();
        if let Some(messages) = list.messages {
            for msg_ref in messages {
                let msg = self.client.get_message(&msg_ref.id).await?;
                emails.push(message_to_email(msg));
            }
        }

        Ok(emails)
    }

    async fn get_message(&self, id: &str) -> Result<Email> {
        let msg = self.client.get_message(id).await?;
        Ok(message_to_email(msg))
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

    async fn remove_label(&self, id: &str, label: &str) -> Result<()> {
        self.client.remove_label(id, label).await
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

fn message_to_email(msg: gmail::Message) -> Email {
    Email {
        id: msg.id.clone(),
        from: msg.get_header("From").unwrap_or("").to_string(),
        to: msg.get_header("To").unwrap_or("").to_string(),
        subject: msg.get_header("Subject").unwrap_or("(no subject)").to_string(),
        body: msg.get_body_text().unwrap_or_default(),
        date: msg.get_header("Date").unwrap_or("").to_string(),
        labels: msg.label_ids.unwrap_or_default(),
    }
}
