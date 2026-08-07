use serde::Deserialize;

use crate::phrases::Snapshot;
use crate::system::{CpuLevel, MemLevel};

const DEFAULT_PERSONA: &str = "あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。\n以下の状況を踏まえて、画面の前のユーザーにかける一言を日本語で 1 文だけ出力してください。\n40 文字以内。軽い挨拶・気遣い・観察など。絵文字・引用符・前置き・説明は不要です。";

fn default_command() -> Vec<String> {
    vec!["claude".to_string(), "-p".to_string()]
}

fn default_probability() -> f64 {
    0.2
}

fn default_timeout() -> u64 {
    30
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    #[serde(default = "default_command")]
    command: Vec<String>,
    #[serde(default = "default_probability")]
    pub probability: f64,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    prompt: Option<String>,
}

impl LlmConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.command.is_empty() {
            anyhow::bail!("[llm] command が空です");
        }
        if !(0.0..=1.0).contains(&self.probability) {
            anyhow::bail!("[llm] probability は 0.0〜1.0 で指定してください");
        }
        if self.timeout_secs == 0 {
            anyhow::bail!("[llm] timeout_secs は 1 以上を指定してください");
        }
        Ok(())
    }
}

/// ペルソナ指示 (既定 or config.prompt) + 状況行からプロンプトを組み立てる
pub fn build_prompt(config: &LlmConfig, now: &Snapshot) -> String {
    let persona = config.prompt.as_deref().unwrap_or(DEFAULT_PERSONA);
    format!(
        "{persona}\n状況: 時間帯={}, 曜日={}, CPU={}, メモリ={}, 連続稼働={}時間",
        crate::phrases::time_band_name(now.hour),
        weekday_name(now.weekday),
        cpu_name(now.cpu),
        mem_name(now.mem),
        now.uptime.as_secs() / 3600,
    )
}

/// CLI 出力から台詞を抽出する。trim → 最初の非空行 → 60 文字切り詰め。空なら None
pub fn postprocess(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(60).collect())
}

fn weekday_name(w: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match w {
        Mon => "mon",
        Tue => "tue",
        Wed => "wed",
        Thu => "thu",
        Fri => "fri",
        Sat => "sat",
        Sun => "sun",
    }
}

fn cpu_name(c: CpuLevel) -> &'static str {
    match c {
        CpuLevel::Idle => "idle",
        CpuLevel::Normal => "normal",
        CpuLevel::High => "high",
    }
}

fn mem_name(m: MemLevel) -> &'static str {
    match m {
        MemLevel::Normal => "normal",
        MemLevel::High => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_snapshot() -> Snapshot {
        Snapshot {
            hour: 6,
            weekday: chrono::Weekday::Mon,
            month: 6,
            day: 15,
            uptime: Duration::from_secs(2 * 3600 + 120),
            cpu: CpuLevel::Idle,
            mem: MemLevel::High,
        }
    }

    #[test]
    fn llm_config_defaults() {
        let cfg: LlmConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.command, vec!["claude", "-p"]);
        assert_eq!(cfg.probability, 0.2);
        assert_eq!(cfg.timeout_secs, 30);
        assert!(cfg.prompt.is_none());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn llm_config_validation_errors() {
        let bad: LlmConfig = toml::from_str("probability = 1.5").unwrap();
        assert!(bad.validate().is_err());
        let bad: LlmConfig = toml::from_str("probability = -0.1").unwrap();
        assert!(bad.validate().is_err());
        let bad: LlmConfig = toml::from_str("command = []").unwrap();
        assert!(bad.validate().is_err());
        let bad: LlmConfig = toml::from_str("timeout_secs = 0").unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn llm_config_rejects_unknown_keys() {
        assert!(toml::from_str::<LlmConfig>("model = \"opus\"").is_err());
    }

    #[test]
    fn build_prompt_uses_default_persona_and_context() {
        let cfg: LlmConfig = toml::from_str("").unwrap();
        let p = build_prompt(&cfg, &test_snapshot());
        assert!(p.starts_with("あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。"));
        assert!(p.ends_with("状況: 時間帯=morning, 曜日=mon, CPU=idle, メモリ=high, 連続稼働=2時間"));
    }

    #[test]
    fn build_prompt_custom_persona() {
        let cfg: LlmConfig = toml::from_str(r#"prompt = "俳句で返して""#).unwrap();
        let p = build_prompt(&cfg, &test_snapshot());
        assert!(p.starts_with("俳句で返して\n状況: "));
    }

    #[test]
    fn postprocess_picks_first_nonempty_line() {
        assert_eq!(
            postprocess("  \n\n  こんにちは  \n二行目").as_deref(),
            Some("こんにちは")
        );
    }

    #[test]
    fn postprocess_truncates_to_60_chars() {
        let long = "あ".repeat(70);
        let got = postprocess(&long).unwrap();
        assert_eq!(got.chars().count(), 60);
    }

    #[test]
    fn postprocess_empty_is_none() {
        assert!(postprocess("").is_none());
        assert!(postprocess("  \n \n").is_none());
    }
}
