use serde::Deserialize;
use crate::phrases::Snapshot;

const DEFAULT_CHAT_PERSONA: &str = "あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。\nユーザーと雑談しています。日本語で、数文・120 文字程度までで自然に返答してください。絵文字・引用符・前置き・説明は不要です。";

/// プロンプトに含める履歴の上限 (発言数 = 10 往復)。ノート保存用の全履歴には影響しない
pub const PROMPT_HISTORY_MAX: usize = 20;
/// チャット返答のハードキャップ (文字数)
pub const REPLY_MAX_CHARS: usize = 300;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    User,
    Mascot,
}

pub struct Turn {
    pub role: Role,
    pub text: String,
}

fn speaker_name(role: Role) -> &'static str {
    match role {
        Role::User => "ユーザー",
        Role::Mascot => "miryam",
    }
}

/// ペルソナ + 状況行 + 直近履歴 (最大 PROMPT_HISTORY_MAX 発言) + 新しい発言
pub fn build_chat_prompt(
    cfg: &ChatConfig,
    turns: &[Turn],
    user_input: &str,
    now: &Snapshot,
) -> String {
    let persona = cfg.prompt.as_deref().unwrap_or(DEFAULT_CHAT_PERSONA);
    let mut out = format!("{persona}\n{}\n", crate::llm::situation_line(now));
    let recent = &turns[turns.len().saturating_sub(PROMPT_HISTORY_MAX)..];
    if !recent.is_empty() {
        out.push_str("これまでの会話:\n");
        for t in recent {
            out.push_str(&format!("{}: {}\n", speaker_name(t.role), t.text));
        }
    }
    out.push_str(&format!("ユーザー: {user_input}\nmiryam:"));
    out
}

/// チャット返答の後処理: 全体 trim → 300 字キャップ → 空なら None。複数行は保持する
/// (自動発話用 llm::postprocess の「最初の 1 行・60 字」とは別物)
pub fn postprocess_chat(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(REPLY_MAX_CHARS).collect())
}

/// 返答の長さに応じた吹き出し表示秒数: 6 + 文字数/10 (上限 20)
pub fn bubble_secs(text: &str) -> u64 {
    (6 + text.chars().count() as u64 / 10).min(20)
}

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
    use crate::phrases::Snapshot;
    use crate::system::{CpuLevel, MemLevel};
    use std::time::Duration;

    fn test_snapshot() -> Snapshot {
        Snapshot {
            hour: 6,
            weekday: chrono::Weekday::Mon,
            month: 6,
            day: 15,
            uptime: Duration::from_secs(2 * 3600),
            cpu: CpuLevel::Idle,
            mem: MemLevel::High,
        }
    }

    fn turn(role: Role, text: &str) -> Turn {
        Turn { role, text: text.to_string() }
    }

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

    #[test]
    fn build_chat_prompt_without_history() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        let p = build_chat_prompt(&cfg, &[], "やあ", &test_snapshot());
        assert!(p.starts_with("あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。"));
        assert!(p.contains("状況: 時間帯=morning, 曜日=mon, CPU=idle, メモリ=high, 連続稼働=2時間"));
        assert!(!p.contains("これまでの会話:"), "履歴なしではブロックを出さない");
        assert!(p.ends_with("ユーザー: やあ\nmiryam:"));
    }

    #[test]
    fn build_chat_prompt_with_history() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        let turns = vec![turn(Role::User, "一言目"), turn(Role::Mascot, "返答一")];
        let p = build_chat_prompt(&cfg, &turns, "二言目", &test_snapshot());
        assert!(p.contains("これまでの会話:\nユーザー: 一言目\nmiryam: 返答一\n"));
        assert!(p.ends_with("ユーザー: 二言目\nmiryam:"));
    }

    #[test]
    fn build_chat_prompt_truncates_history_to_last_20() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        // 15 往復 = 30 発言。user1..user15 / reply1..reply15
        let mut turns = Vec::new();
        for i in 1..=15 {
            turns.push(turn(Role::User, &format!("user{i}")));
            turns.push(turn(Role::Mascot, &format!("reply{i}")));
        }
        let p = build_chat_prompt(&cfg, &turns, "next", &test_snapshot());
        assert!(!p.contains("ユーザー: user5\n"), "古い発言 (先頭 10 件) は落ちる");
        assert!(p.contains("ユーザー: user6\n"), "直近 20 発言の先頭は user6");
        assert!(p.contains("miryam: reply15\n"));
    }

    #[test]
    fn build_chat_prompt_custom_persona() {
        let cfg: ChatConfig = toml::from_str(r#"prompt = "関西弁で話して""#).unwrap();
        let p = build_chat_prompt(&cfg, &[], "やあ", &test_snapshot());
        assert!(p.starts_with("関西弁で話して\n状況: "));
    }

    #[test]
    fn postprocess_chat_keeps_multiline_and_trims() {
        assert_eq!(
            postprocess_chat("  こんにちは。\n今日もいい天気ですね。\n").as_deref(),
            Some("こんにちは。\n今日もいい天気ですね。")
        );
    }

    #[test]
    fn postprocess_chat_caps_at_300_chars() {
        let long = "あ".repeat(400);
        assert_eq!(postprocess_chat(&long).unwrap().chars().count(), 300);
    }

    #[test]
    fn postprocess_chat_empty_is_none() {
        assert!(postprocess_chat("").is_none());
        assert!(postprocess_chat("  \n \n").is_none());
    }

    #[test]
    fn bubble_secs_scales_with_length() {
        assert_eq!(bubble_secs("短い"), 6);
        assert_eq!(bubble_secs(&"あ".repeat(100)), 16);
        assert_eq!(bubble_secs(&"あ".repeat(140)), 20);
        assert_eq!(bubble_secs(&"あ".repeat(300)), 20, "上限 20 秒");
    }
}
