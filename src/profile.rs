use crate::config;
use anyhow::Result;
use std::fs;

const DEFAULT_PROFILE: &str = r#"# Email Classification Profile

## Spam Patterns
- (Add patterns as you mark emails as spam)

## Important Signals
- Emails mentioning my name directly in body are important
- Replies to emails I sent are important

## Label Rules

## Learned Corrections
"#;

pub struct Profile {
    content: String,
}

impl Profile {
    pub fn load() -> Result<Self> {
        let path = config::profile_path();
        let content = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            DEFAULT_PROFILE.to_string()
        };
        Ok(Self { content })
    }

    pub fn save(&self) -> Result<()> {
        let dir = config::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        fs::write(config::profile_path(), &self.content)?;
        Ok(())
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn update(&mut self, new_content: String) {
        self.content = new_content;
    }

    pub fn append_correction(&mut self, correction: &str) {
        // Find the "## Learned Corrections" section and append
        if let Some(idx) = self.content.find("## Learned Corrections") {
            let insert_pos = self.content[idx..].find('\n').map(|i| idx + i + 1).unwrap_or(self.content.len());
            self.content.insert_str(insert_pos, &format!("- {}\n", correction));
        } else {
            // Add section if it doesn't exist
            self.content.push_str(&format!("\n## Learned Corrections\n- {}\n", correction));
        }
    }

    pub fn remove_label_rules(&mut self, label: &str) {
        // Remove a label section from profile
        let section_header = format!("### {}", label);
        if let Some(start) = self.content.find(&section_header) {
            // Find the next section or end
            let remaining = &self.content[start + section_header.len()..];
            let end = remaining.find("\n### ")
                .or_else(|| remaining.find("\n## "))
                .map(|i| start + section_header.len() + i)
                .unwrap_or(self.content.len());

            self.content = format!(
                "{}{}",
                &self.content[..start],
                &self.content[end..]
            );
        }
    }
}
