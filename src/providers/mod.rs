pub mod gmail;
pub mod outlook;
pub mod outlook_web;

use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct Email {
    pub id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Label {
    pub id: String,
    pub name: String,
}

#[async_trait]
pub trait EmailProvider: Send + Sync {
    async fn list_messages(&self, max: u32, label: &str, query: Option<&str>) -> Result<Vec<Email>>;
    async fn get_message(&self, id: &str) -> Result<Email>;
    async fn list_labels(&self) -> Result<Vec<Label>>;
    async fn add_label(&self, id: &str, label: &str) -> Result<()>;
    async fn mark_spam(&self, id: &str) -> Result<()>;
    async fn unspam(&self, id: &str) -> Result<()>;
    async fn archive(&self, id: &str) -> Result<()>;
    async fn trash(&self, id: &str) -> Result<()>;
}

#[async_trait]
impl EmailProvider for Box<dyn EmailProvider> {
    async fn list_messages(&self, max: u32, label: &str, query: Option<&str>) -> Result<Vec<Email>> {
        (**self).list_messages(max, label, query).await
    }
    async fn get_message(&self, id: &str) -> Result<Email> {
        (**self).get_message(id).await
    }
    async fn list_labels(&self) -> Result<Vec<Label>> {
        (**self).list_labels().await
    }
    async fn add_label(&self, id: &str, label: &str) -> Result<()> {
        (**self).add_label(id, label).await
    }
    async fn mark_spam(&self, id: &str) -> Result<()> {
        (**self).mark_spam(id).await
    }
    async fn unspam(&self, id: &str) -> Result<()> {
        (**self).unspam(id).await
    }
    async fn archive(&self, id: &str) -> Result<()> {
        (**self).archive(id).await
    }
    async fn trash(&self, id: &str) -> Result<()> {
        (**self).trash(id).await
    }
}
