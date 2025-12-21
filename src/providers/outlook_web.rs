use super::{Email, EmailProvider, Label};
use anyhow::Result;
use async_trait::async_trait;

pub struct OutlookWebProvider {
    client: outlook_web::api::Client,
}

impl OutlookWebProvider {
    pub fn new() -> Result<Self> {
        let cfg = outlook_web::config::load_config()?;
        let client = outlook_web::api::Client::new(cfg.port());
        Ok(Self { client })
    }

    fn message_to_email(&self, msg: outlook_web::api::Message) -> Email {
        let mut labels = msg.labels.clone();

        // Add INBOX pseudo-label (outlook-web list defaults to inbox)
        labels.push("INBOX".to_string());

        // Add UNREAD pseudo-label if unread
        if msg.is_unread {
            labels.push("UNREAD".to_string());
        }

        Email {
            id: msg.id,
            from: msg.from.unwrap_or_default(),
            to: String::new(), // outlook-web doesn't expose To field
            subject: msg.subject.unwrap_or_else(|| "(no subject)".to_string()),
            body: msg.body.or(msg.preview).unwrap_or_default(),
            labels,
        }
    }
}

#[async_trait]
impl EmailProvider for OutlookWebProvider {
    async fn list_messages(&self, max: u32, label: &str, query: Option<&str>) -> Result<Vec<Email>> {
        // outlook-web only supports inbox for now
        if label != "INBOX" && !label.is_empty() {
            return Ok(Vec::new());
        }

        let messages = self.client.list_messages(max).await?;

        let mut emails: Vec<Email> = messages
            .into_iter()
            .map(|msg| self.message_to_email(msg))
            .collect();

        // Apply query filter if present (e.g., "-label:Classified")
        if let Some(q) = query {
            if q.starts_with("-label:") {
                let excluded_label = q.trim_start_matches("-label:");
                emails.retain(|e| !e.labels.iter().any(|l| l == excluded_label));
            }
        }

        Ok(emails)
    }

    async fn get_message(&self, id: &str) -> Result<Email> {
        let msg = self.client.get_message(id).await?;
        Ok(self.message_to_email(msg))
    }

    async fn list_labels(&self) -> Result<Vec<Label>> {
        let label_names = self.client.list_labels().await?;
        Ok(label_names
            .into_iter()
            .map(|name| Label {
                id: name.clone(),
                name,
            })
            .collect())
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
