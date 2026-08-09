use serde::Deserialize;

fn default_command() -> Vec<String> {
    vec!["claude".to_string(), "-p".to_string()]
}

fn default_timeout() -> u64 {
    60
}

fn default_idle_close() -> u64 {
    600
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatConfig {
    #[serde(default = "default_command")]
    pub command: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_idle_close")]
    pub idle_close_secs: u64,
    #[serde(default)]
    prompt: Option<String>,
}

impl ChatConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.command.is_empty() {
            anyhow::bail!("[chat] command が空です");
        }
        if self.timeout_secs == 0 {
            anyhow::bail!("[chat] timeout_secs は 1 以上を指定してください");
        }
        if self.idle_close_secs == 0 {
            anyhow::bail!("[chat] idle_close_secs は 1 以上を指定してください");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_config_defaults() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.command, vec!["claude", "-p"]);
        assert_eq!(cfg.timeout_secs, 60);
        assert_eq!(cfg.idle_close_secs, 600);
        assert!(cfg.prompt.is_none());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn chat_config_validation_errors() {
        let bad: ChatConfig = toml::from_str("command = []").unwrap();
        assert!(bad.validate().is_err());
        let bad: ChatConfig = toml::from_str("timeout_secs = 0").unwrap();
        assert!(bad.validate().is_err());
        let bad: ChatConfig = toml::from_str("idle_close_secs = 0").unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn chat_config_rejects_unknown_keys() {
        assert!(toml::from_str::<ChatConfig>("model = \"opus\"").is_err());
    }
}
