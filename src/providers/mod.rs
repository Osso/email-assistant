pub mod gmail;

use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct Email {
    pub id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub date: String,
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
    async fn remove_label(&self, id: &str, label: &str) -> Result<()>;
    async fn mark_spam(&self, id: &str) -> Result<()>;
    async fn unspam(&self, id: &str) -> Result<()>;
    async fn archive(&self, id: &str) -> Result<()>;
    async fn trash(&self, id: &str) -> Result<()>;
}
