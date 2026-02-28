use std::collections::BTreeMap;

use crate::config::{CommandEntry, FireConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionSuggestion {
    pub(crate) value: String,
    pub(crate) description: Option<String>,
}

pub(crate) fn completion_suggestions(
    config: &FireConfig,
    words: &[String],
) -> Vec<CompletionSuggestion> {
    if words.is_empty() {
        return suggestions_with_prefix("", &config.commands);
    }

    let mut available = &config.commands;

    for token in &words[..words.len() - 1] {
        let Some(entry) = available.get(token) else {
            return Vec::new();
        };
        let Some(subcommands) = entry.subcommands() else {
            return Vec::new();
        };
        available = subcommands;
    }

    let prefix = words.last().map(String::as_str).unwrap_or("");

    if let Some(exact) = available.get(prefix) {
        if let Some(subcommands) = exact.subcommands() {
            return suggestions_with_prefix("", subcommands);
        }
        return Vec::new();
    }

    suggestions_with_prefix(prefix, available)
}

pub(crate) fn suggestions_with_prefix(
    prefix: &str,
    commands: &BTreeMap<String, CommandEntry>,
) -> Vec<CompletionSuggestion> {
    commands
        .iter()
        .filter_map(|(name, entry)| {
            if !name.starts_with(prefix) {
                return None;
            }
            let description = entry.description().unwrap_or_default().trim();
            if description.is_empty() {
                Some(CompletionSuggestion {
                    value: name.clone(),
                    description: None,
                })
            } else {
                Some(CompletionSuggestion {
                    value: name.clone(),
                    description: Some(description.to_string()),
                })
            }
        })
        .collect()
}

pub(crate) fn render_with_descriptions(suggestions: &[CompletionSuggestion]) -> Vec<String> {
    suggestions
        .iter()
        .map(|suggestion| match suggestion.description.as_deref() {
            Some(description) => format!(
                "{}\t{}",
                suggestion.value,
                first_description_line(description)
            ),
            None => suggestion.value.clone(),
        })
        .collect()
}

pub(crate) fn render_values_only(suggestions: &[CompletionSuggestion]) -> Vec<String> {
    suggestions
        .iter()
        .map(|suggestion| suggestion.value.clone())
        .collect()
}

fn first_description_line(description: &str) -> &str {
    description.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::config::{CommandEntry, CommandSpec, FireConfig};

    use super::*;

    #[test]
    fn completion_includes_description() {
        let mut map = BTreeMap::new();
        map.insert(
            "run".to_string(),
            CommandEntry::Spec(CommandSpec {
                description: "run scripts".to_string(),
                ..CommandSpec::default()
            }),
        );
        map.insert(
            "raw".to_string(),
            CommandEntry::Shorthand("echo raw".to_string()),
        );

        let values = suggestions_with_prefix("r", &map);
        assert_eq!(
            values,
            vec![
                CompletionSuggestion {
                    value: "raw".to_string(),
                    description: None
                },
                CompletionSuggestion {
                    value: "run".to_string(),
                    description: Some("run scripts".to_string())
                }
            ]
        );
    }

    #[test]
    fn completion_moves_into_subcommands_on_exact_match() {
        let yaml = r#"
commands:
  vars:
    commands:
      npm-version: echo ok
      node-version: echo ok
"#;
        let config: FireConfig = serde_yaml::from_str(yaml).expect("valid config");
        let words = vec!["vars".to_string()];

        let values = completion_suggestions(&config, &words);
        assert_eq!(
            values,
            vec![
                CompletionSuggestion {
                    value: "node-version".to_string(),
                    description: None
                },
                CompletionSuggestion {
                    value: "npm-version".to_string(),
                    description: None
                }
            ]
        );
    }

    #[test]
    fn completion_returns_empty_for_exact_command_without_subcommands() {
        let yaml = r#"
commands:
  run: npm run
  run2: npm run test
"#;
        let config: FireConfig = serde_yaml::from_str(yaml).expect("valid config");
        let words = vec!["run".to_string()];

        let values = completion_suggestions(&config, &words);
        assert!(values.is_empty());
    }

    #[test]
    fn render_with_descriptions_uses_only_first_line_of_description() {
        let suggestions = vec![CompletionSuggestion {
            value: "run".to_string(),
            description: Some("run service\nwith custom host".to_string()),
        }];

        let rendered = render_with_descriptions(&suggestions);
        assert_eq!(rendered, vec!["run\trun service".to_string()]);
    }
}
