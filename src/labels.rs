use crate::config;
use crate::profile::Profile;
use crate::providers::EmailProvider;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelInfo {
    pub name: String,
    pub source: LabelSource,
    pub email_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LabelSource {
    Provider,
    Llm,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LabelManager {
    labels: HashMap<String, LabelInfo>,
}

impl LabelManager {
    pub fn load() -> Result<Self> {
        let path = config::labels_path();
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = config::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(config::labels_path(), content)?;
        Ok(())
    }

    pub fn llm_labels(&self) -> Vec<&str> {
        self.labels
            .values()
            .filter(|l| l.source == LabelSource::Llm)
            .map(|l| l.name.as_str())
            .collect()
    }

    pub async fn cleanup<P: EmailProvider>(
        &mut self,
        provider: &P,
        profile: &mut Profile,
    ) -> Result<Vec<String>> {
        let mut removed = Vec::new();

        // Get LLM labels that need checking
        let llm_labels: Vec<String> = self
            .labels
            .values()
            .filter(|l| l.source == LabelSource::Llm)
            .map(|l| l.name.clone())
            .collect();

        for label_name in llm_labels {
            // Query provider for emails with this label
            let emails = provider.list_messages(1, &label_name, None).await;

            match emails {
                Ok(emails) if emails.is_empty() => {
                    // No emails with this label, remove it
                    self.labels.remove(&label_name);
                    profile.remove_label_rules(&label_name);
                    removed.push(label_name);
                }
                Err(_) => {
                    // Label doesn't exist in provider, remove it
                    self.labels.remove(&label_name);
                    profile.remove_label_rules(&label_name);
                    removed.push(label_name);
                }
                _ => {}
            }
        }

        Ok(removed)
    }
}
