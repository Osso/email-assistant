# email-assistant

AI-powered email classification using Claude. Automatically labels and organizes emails based on customizable rules, learns from your corrections.

## Installation

```bash
# Clone and build
git clone https://github.com/Osso/email-assistant.git
cd email-assistant
cargo install --path .

# Authenticate with your email provider (opens browser)
email-assistant login                      # Gmail (default)
email-assistant --provider outlook login   # Outlook
```

Requires [Claude Code CLI](https://claude.ai/code) to be installed and authenticated.

## Usage

```bash
# Scan and classify inbox emails
email-assistant scan

# Scan with limit
email-assistant scan -n 100

# Use Outlook instead of Gmail
email-assistant --provider outlook scan

# Dry run (show what would happen)
email-assistant --dry-run scan

# Get AI summary of inbox
email-assistant summary

# Show emails needing reply
email-assistant needs-reply

# Learn from your corrections
email-assistant learn

# Show classification profile
email-assistant profile
```

## Commands

| Command | Description |
|---------|-------------|
| `login` | Authenticate with email provider |
| `scan` | Classify unprocessed emails |
| `summary` | AI-generated inbox summary |
| `learn` | Learn from label corrections |
| `needs-reply` | Show emails awaiting response |
| `profile` | Show classification rules |
| `labels` | List all labels |
| `labels cleanup` | Remove empty labels |
| `spam <id>` | Mark as spam |
| `unspam <id>` | Remove from spam |
| `archive <id>` | Archive email |
| `delete <id>` | Move to trash |
| `label <id> <label>` | Add label |

## Configuration

Classification profile is stored at `~/.config/email-assistant/profile.md`. Edit this file to customize classification rules.

Custom rules can be added in `~/.config/email-assistant/rules/` as JSON files:

```json
{
  "rules": [
    {
      "name": "Archive work emails",
      "condition": {
        "field": "to",
        "contains": "work@example.com",
        "and": "archive"
      },
      "action": "delete"
    }
  ]
}
```

## License

MIT
