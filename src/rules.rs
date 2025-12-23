use crate::classifier::Classification;
use crate::config;
use crate::providers::Email;
use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct RuleFile {
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    #[allow(dead_code)] // Used for config documentation
    pub name: String,
    #[serde(default)]
    #[allow(dead_code)] // Used for config documentation
    pub description: String,
    pub condition: Condition,
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub struct Condition {
    pub field: String,
    pub contains: String,
    /// Additional condition: "archive" means only apply if classification.archive is true
    #[serde(default)]
    pub and: Option<String>,
}

fn rules_dir() -> PathBuf {
    config::config_dir().join("rules")
}

pub fn load_rules() -> Result<Vec<Rule>> {
    let dir = rules_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut all_rules = Vec::new();

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            let content = fs::read_to_string(&path)?;
            let rule_file: RuleFile = serde_json::from_str(&content)?;
            all_rules.extend(rule_file.rules);
        }
    }

    Ok(all_rules)
}

/// Apply rules to override classification
pub fn apply_rules(email: &Email, classification: &mut Classification, rules: &[Rule]) {
    for rule in rules {
        if matches_condition(email, classification, &rule.condition) {
            match rule.action.as_str() {
                "delete" => {
                    classification.delete = true;
                    classification.archive = false;
                }
                "archive" => {
                    classification.archive = true;
                }
                _ => {}
            }
        }
    }
}

fn matches_condition(email: &Email, classification: &Classification, condition: &Condition) -> bool {
    // Check the field condition
    let field_matches = match condition.field.as_str() {
        "to" => email.to.to_lowercase().contains(&condition.contains.to_lowercase()),
        "from" => email.from.to_lowercase().contains(&condition.contains.to_lowercase()),
        "subject" => email.subject.to_lowercase().contains(&condition.contains.to_lowercase()),
        _ => false,
    };

    if !field_matches {
        return false;
    }

    // Check additional condition if present
    if let Some(ref and_condition) = condition.and {
        match and_condition.as_str() {
            "archive" => classification.archive,
            "delete" => classification.delete,
            _ => true,
        }
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_email(to: &str) -> Email {
        Email {
            id: "test123".to_string(),
            from: "sender@example.com".to_string(),
            to: to.to_string(),
            subject: "Test Subject".to_string(),
            body: "Test body".to_string(),
            labels: vec![],
        }
    }

    fn make_classification(archive: bool) -> Classification {
        Classification {
            is_spam: false,
            archive,
            delete: false,
            theme: vec![],
            action: vec![],
            confidence: 0.9,
        }
    }

    fn globalcomix_rule() -> Rule {
        Rule {
            name: "Delete globalcomix".to_string(),
            description: "Test rule".to_string(),
            condition: Condition {
                field: "to".to_string(),
                contains: "globalcomix.com".to_string(),
                and: Some("archive".to_string()),
            },
            action: "delete".to_string(),
        }
    }

    #[test]
    fn test_delete_rule_triggers_when_archive_true() {
        let email = make_email("user@globalcomix.com");
        let mut classification = make_classification(true);
        let rules = vec![globalcomix_rule()];

        apply_rules(&email, &mut classification, &rules);

        assert!(classification.delete);
        assert!(!classification.archive);
    }

    #[test]
    fn test_delete_rule_skipped_when_archive_false() {
        let email = make_email("user@globalcomix.com");
        let mut classification = make_classification(false);
        let rules = vec![globalcomix_rule()];

        apply_rules(&email, &mut classification, &rules);

        assert!(!classification.delete);
        assert!(!classification.archive);
    }

    #[test]
    fn test_delete_rule_skipped_for_other_domains() {
        let email = make_email("user@example.com");
        let mut classification = make_classification(true);
        let rules = vec![globalcomix_rule()];

        apply_rules(&email, &mut classification, &rules);

        assert!(!classification.delete);
        assert!(classification.archive);
    }

    #[test]
    fn test_case_insensitive_match() {
        let email = make_email("User@GLOBALCOMIX.COM");
        let mut classification = make_classification(true);
        let rules = vec![globalcomix_rule()];

        apply_rules(&email, &mut classification, &rules);

        assert!(classification.delete);
    }
}
