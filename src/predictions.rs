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
    pub is_spam: bool,
    pub is_important: bool,
    pub labels: Vec<String>,
    pub confidence: f32,
    pub timestamp: DateTime<Utc>,
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

    pub fn store(&mut self, email_id: &str, classification: &Classification) -> Result<()> {
        self.predictions.insert(
            email_id.to_string(),
            Prediction {
                email_id: email_id.to_string(),
                is_spam: classification.is_spam,
                is_important: classification.is_important,
                labels: classification.labels.clone(),
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
