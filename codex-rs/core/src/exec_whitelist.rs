use std::fs;
use std::path::Path;

use crate::bash::try_parse_bash;
use crate::bash::try_parse_word_only_commands_sequence;
use regex_lite::Regex;
use serde::Deserialize;

/// Repository-level exec whitelist loaded from `.codex/exec_whitelist.toml`.
///
/// Example TOML:
///
/// ```toml
/// [[whitelist]]
/// prefix = ["brew", "install"]
///
/// [[whitelist]]
/// prefix = ["aws", "s3", "cp"]
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExecWhitelistToml {
    #[serde(default)]
    pub whitelist: Vec<WhitelistRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WhitelistRule {
    /// Command must start with this argv prefix to be whitelisted.
    #[serde(default)]
    pub prefix: Vec<String>,

    /// Regex-based token prefix. Each pattern is applied to the corresponding
    /// argv token (from the start). All must match to whitelist the command.
    /// Example: ["^brew$", "^inst.*$"]
    #[serde(default)]
    pub regex_tokens: Vec<String>,
}

impl ExecWhitelistToml {
    /// Load from `<repo_root>/.codex/exec_whitelist.toml` if present.
    pub fn load_from_repo_root(repo_root: &Path) -> Self {
        let path = repo_root.join(".codex").join("exec_whitelist.toml");
        let Ok(bytes) = fs::read(&path) else {
            return ExecWhitelistToml::default();
        };
        toml::from_slice::<ExecWhitelistToml>(&bytes).unwrap_or_default()
    }

    /// Return true if any rule matches `argv` by prefix.
    pub fn matches(&self, argv: &[String]) -> bool {
        if self.whitelist.is_empty() || argv.is_empty() {
            return false;
        }

        // If the command is a shell wrapper like `bash -lc "..."`, only allow
        // whitelisting when the script consists of exactly one plain command
        // (no operators) and evaluate rules against that command's argv.
        let argv_effective: std::borrow::Cow<'_, [String]> = if let [bash, flag, script] = argv
            && bash == "bash"
            && flag == "-lc"
            && let Some(tree) = try_parse_bash(script)
            && let Some(all_commands) = try_parse_word_only_commands_sequence(&tree, script)
            && all_commands.len() == 1
        {
            std::borrow::Cow::Owned(all_commands[0].clone())
        } else {
            std::borrow::Cow::Borrowed(argv)
        };

        for rule in &self.whitelist {
            // Exact prefix match
            if !rule.prefix.is_empty() {
                if rule.prefix.len() <= argv_effective.len()
                    && rule
                        .prefix
                        .iter()
                        .zip(argv_effective.iter())
                        .all(|(want, got)| want == got)
                {
                    return true;
                }
            }

            // Regex token prefix match
            if !rule.regex_tokens.is_empty() {
                if rule.regex_tokens.len() <= argv_effective.len() {
                    let mut all_match = true;
                    for (pat, got) in rule.regex_tokens.iter().zip(argv_effective.iter()) {
                        // Anchor patterns to require full-token match.
                        let wrapped = format!("^(?:{pat})$");
                        let Ok(re) = Regex::new(&wrapped) else {
                            all_match = false;
                            break;
                        };
                        if !re.is_match(got) {
                            all_match = false;
                            break;
                        }
                    }
                    if all_match {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_matches_prefix() {
        let wl = ExecWhitelistToml {
            whitelist: vec![WhitelistRule {
                prefix: v(&["brew", "install"]),
                regex_tokens: vec![],
            }],
        };
        assert!(wl.matches(&v(&["brew", "install", "ripgrep"])));
        assert!(!wl.matches(&v(&["brew"])));
        assert!(!wl.matches(&v(&["brew", "uninstall"])));
    }

    #[test]
    fn test_matches_regex_tokens() {
        let wl = ExecWhitelistToml {
            whitelist: vec![WhitelistRule {
                prefix: vec![],
                regex_tokens: v(&["^brew$", "^inst.*$"]),
            }],
        };
        assert!(wl.matches(&v(&["brew", "install", "ripgrep"])));
        assert!(!wl.matches(&v(&["brew", "uninstall"])));
        assert!(!wl.matches(&v(&["brewer", "install"])));
    }
}
