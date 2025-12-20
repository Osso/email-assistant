use crate::classifier::Classification;
use crate::config;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Serialize, Deserialize)]
pub struct Prediction {
    pub email_id: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub subject: String,
    pub is_spam: bool,
    /// Theme labels (what email is about)
    #[serde(default)]
    pub theme: Vec<String>,
    /// Action labels (what to do with it)
    #[serde(default)]
    pub action: Vec<String>,
    /// Legacy field for backward compatibility
    #[serde(default)]
    pub labels: Vec<String>,
    pub confidence: f32,
    pub timestamp: DateTime<Utc>,
}

impl Prediction {
    /// All labels combined
    pub fn all_labels(&self) -> Vec<String> {
        if !self.theme.is_empty() || !self.action.is_empty() {
            self.theme.iter().chain(self.action.iter()).cloned().collect()
        } else {
            // Fallback to legacy labels field
            self.labels.clone()
        }
    }

    pub fn is_important(&self) -> bool {
        self.action.iter().any(|a| a == "Important" || a == "Urgent")
    }

    pub fn needs_reply(&self) -> bool {
        self.action.iter().any(|a| a == "Needs-Reply")
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PredictionStore {
    predictions: HashMap<String, Prediction>,
}

impl PredictionStore {
    pub fn load() -> Result<Self> {
        let path = config::predictions_path();
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
        fs::write(config::predictions_path(), content)?;
        Ok(())
    }

    pub fn has_prediction(&self, email_id: &str) -> bool {
        self.predictions.contains_key(email_id)
    }

    pub fn get(&self, email_id: &str) -> Option<&Prediction> {
        self.predictions.get(email_id)
    }

    pub fn store(&mut self, email_id: &str, from: &str, subject: &str, classification: &Classification) -> Result<()> {
        self.predictions.insert(
            email_id.to_string(),
            Prediction {
                email_id: email_id.to_string(),
                from: from.to_string(),
                subject: subject.to_string(),
                is_spam: classification.is_spam,
                theme: classification.theme.clone(),
                action: classification.action.clone(),
                labels: vec![], // Legacy field, no longer used
                confidence: classification.confidence,
                timestamp: Utc::now(),
            },
        );
        Ok(())
    }

    pub fn remove(&mut self, email_id: &str) {
        self.predictions.remove(email_id);
    }

    pub fn all_predictions(&self) -> impl Iterator<Item = &Prediction> {
        self.predictions.values()
    }
}
