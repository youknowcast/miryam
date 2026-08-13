use crate::phrases::Snapshot;
use serde::Deserialize;

const DEFAULT_CHAT_PERSONA: &str = "あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。\nユーザーと雑談しています。日本語で、数文・120 文字程度までで自然に返答してください。絵文字・引用符・前置き・説明は不要です。";

const DESIGN_PERSONA: &str = "あなたはデスクトップに常駐する小さなマスコット「miryam」です。\nユーザーとソフトウェア設計の議論をしています。壁打ち相手として、論点の整理・前提の確認・トレードオフの提示を簡潔に行ってください。結論を急がず、ユーザーの考えを引き出す問いを 1 つ添えてください。日本語で 400 文字程度まで。絵文字・前置きは不要です。";

const ENGLISH_PERSONA: &str = "あなたはデスクトップに常駐する小さなマスコット「miryam」です。\nユーザーの「これ英語でなんて言う?」に答えています。自然な英語表現を 1〜3 個、それぞれ短い例文とニュアンスの日本語解説付きで示してください。400 文字程度まで。絵文字・前置きは不要です。";

/// 吹き出し表示される組み込みモード名 (これ以外のモードは常に会話窓)
pub const CASUAL_MODE_NAME: &str = "雑談";

/// 会話窓モードでペルソナ直後に挿入する選択肢マーカー指示
const CHOICES_INSTRUCTION: &str = "返答の本文の後に、ユーザーが次に選びそうな一手を「>> 候補」の形式で 1 行 1 候補、最大 3 行出力してください。候補が思いつかなければ出力しなくて構いません。";

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

/// 会話セッション。不変条件: turns は成立した往復のみ
/// (User → Mascot の交互ペア。送信直後のユーザー発言はここに入れず、
/// 返答が来た時に push_exchange で 2 発言まとめて積む — pending 方式)
pub struct ChatSession {
    pub turns: Vec<Turn>,
    pub started_at: chrono::DateTime<chrono::Local>,
    /// Some = 会話ウィンドウモード。None = 雑談 (吹き出し)
    pub mode: Option<ChatMode>,
}

impl ChatSession {
    pub fn new(started_at: chrono::DateTime<chrono::Local>) -> Self {
        Self {
            turns: Vec::new(),
            started_at,
            mode: None,
        }
    }

    /// 会話ウィンドウモードのセッションを開始する
    pub fn with_mode(started_at: chrono::DateTime<chrono::Local>, mode: ChatMode) -> Self {
        Self {
            turns: Vec::new(),
            started_at,
            mode: Some(mode),
        }
    }

    /// 成立した往復を積む (返答が来た時だけ呼ぶこと)
    pub fn push_exchange(&mut self, user: String, mascot: String) {
        self.turns.push(Turn {
            role: Role::User,
            text: user,
        });
        self.turns.push(Turn {
            role: Role::Mascot,
            text: mascot,
        });
    }
}

/// セッションを Inkdrop ノート (title, body) に整形する
pub fn chat_note(
    session: &ChatSession,
    ended_at: &chrono::DateTime<chrono::Local>,
) -> (String, String) {
    let stamp = session.started_at.format("%Y-%m-%d %H:%M");
    let title = match &session.mode {
        Some(m) => format!("会話ログ ({}) {stamp}", m.name),
        None => format!("会話ログ {stamp}"),
    };
    let mut blocks: Vec<String> = Vec::new();
    for pair in session.turns.chunks(2) {
        let mut block = String::new();
        for t in pair {
            block.push_str(&format!("**{}**: {}\n", speaker_name(t.role), t.text));
        }
        blocks.push(block);
    }
    let body = format!(
        "{}\n---\n- 開始: {}\n- 終了: {}\nSource: miryam chat",
        blocks.join("\n"),
        session.started_at.format("%Y-%m-%d %H:%M"),
        ended_at.format("%Y-%m-%d %H:%M"),
    );
    (title, body)
}

/// ペルソナ + (選択肢指示) + 状況行 + 直近履歴 + 新しい発言
fn assemble_prompt(
    persona: &str,
    with_choices: bool,
    turns: &[Turn],
    user_input: &str,
    now: &Snapshot,
) -> String {
    let mut out = format!("{persona}\n");
    if with_choices {
        out.push_str(CHOICES_INSTRUCTION);
        out.push('\n');
    }
    out.push_str(&crate::llm::situation_line(now));
    out.push('\n');
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

/// ペルソナ + 状況行 + 直近履歴 (最大 PROMPT_HISTORY_MAX 発言) + 新しい発言 (雑談・吹き出し用)
pub fn build_chat_prompt(
    cfg: &ChatConfig,
    turns: &[Turn],
    user_input: &str,
    now: &Snapshot,
) -> String {
    let persona = cfg.prompt.as_deref().unwrap_or(DEFAULT_CHAT_PERSONA);
    assemble_prompt(persona, false, turns, user_input, now)
}

/// モードのペルソナでプロンプトを組み立てる。会話窓モードは選択肢指示付き
pub fn build_mode_prompt(
    mode: &ChatMode,
    turns: &[Turn],
    user_input: &str,
    now: &Snapshot,
) -> String {
    assemble_prompt(&mode.prompt, mode.window, turns, user_input, now)
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

/// 会話窓モードの選択肢マーカー (行頭)
const CHOICE_MARKER: &str = ">>";
/// 1 返答あたりの選択肢ボタン上限
pub const CHOICES_MAX: usize = 3;
/// 会話窓モード返答のハードキャップ (文字数)。吹き出しの REPLY_MAX_CHARS とは別
pub const WINDOW_REPLY_MAX_CHARS: usize = 2000;

/// 返答を本文と選択肢に分離する。行頭 (trim 後) が ">>" の行を選択肢として抽出し、
/// 最大 CHOICES_MAX 個まで採用する。マーカーが 1 つもなければ全文が本文 —
/// 形式が崩れても本文は失わない
pub fn split_choices(text: &str) -> (String, Vec<String>) {
    let mut body_lines: Vec<&str> = Vec::new();
    let mut choices: Vec<String> = Vec::new();
    for line in text.lines() {
        match line.trim_start().strip_prefix(CHOICE_MARKER) {
            Some(rest) => {
                let choice = rest.trim();
                if !choice.is_empty() && choices.len() < CHOICES_MAX {
                    choices.push(choice.to_string());
                }
            }
            None => body_lines.push(line),
        }
    }
    (body_lines.join("\n").trim().to_string(), choices)
}

/// 会話窓モードの返答後処理: 全体 trim → 2000 字キャップ → 空なら None
pub fn postprocess_window(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(WINDOW_REPLY_MAX_CHARS).collect())
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

/// [[chat.mode]] の 1 エントリ (toml 入力)
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatModeConfig {
    name: String,
    prompt: String,
}

/// 実行時のモード定義 (組み込み + [[chat.mode]] マージ済み)
#[derive(Clone, PartialEq, Debug)]
pub struct ChatMode {
    pub name: String,
    pub prompt: String,
    /// true = 会話ウィンドウ表示。false = 吹き出し (雑談専用)
    pub window: bool,
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
    #[serde(default, rename = "mode")]
    modes: Vec<ChatModeConfig>,
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
        let mut seen = std::collections::HashSet::new();
        for m in &self.modes {
            if m.name.trim().is_empty() {
                anyhow::bail!("[[chat.mode]] の name が空です");
            }
            if !seen.insert(m.name.as_str()) {
                anyhow::bail!("[[chat.mode]] の name \"{}\" が重複しています", m.name);
            }
        }
        Ok(())
    }

    /// 組み込み 3 モードに [[chat.mode]] をマージする。同名は prompt 上書き
    /// (表示先は変えない)、新名は会話窓モードとして追加。
    /// 雑談のペルソナ優先順位: [[chat.mode]] > [chat] prompt > 既定
    pub fn modes(&self) -> Vec<ChatMode> {
        let casual_prompt = self
            .prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_CHAT_PERSONA.to_string());
        let mut out = vec![
            ChatMode {
                name: CASUAL_MODE_NAME.to_string(),
                prompt: casual_prompt,
                window: false,
            },
            ChatMode {
                name: "設計議論".to_string(),
                prompt: DESIGN_PERSONA.to_string(),
                window: true,
            },
            ChatMode {
                name: "英語表現".to_string(),
                prompt: ENGLISH_PERSONA.to_string(),
                window: true,
            },
        ];
        for m in &self.modes {
            match out.iter_mut().find(|b| b.name == m.name) {
                Some(builtin) => builtin.prompt = m.prompt.clone(),
                None => out.push(ChatMode {
                    name: m.name.clone(),
                    prompt: m.prompt.clone(),
                    window: true,
                }),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrases::Snapshot;
    use crate::system::{CpuLevel, MemLevel};
    use chrono::TimeZone;
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
        Turn {
            role,
            text: text.to_string(),
        }
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
        assert!(
            p.starts_with("あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。")
        );
        assert!(
            p.contains("状況: 時間帯=morning, 曜日=mon, CPU=idle, メモリ=high, 連続稼働=2時間")
        );
        assert!(
            !p.contains("これまでの会話:"),
            "履歴なしではブロックを出さない"
        );
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
        assert!(
            !p.contains("ユーザー: user5\n"),
            "古い発言 (先頭 10 件) は落ちる"
        );
        assert!(
            p.contains("ユーザー: user6\n"),
            "直近 20 発言の先頭は user6"
        );
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

    fn session_at(h: u32, m: u32) -> ChatSession {
        ChatSession::new(chrono::Local.with_ymd_and_hms(2026, 8, 9, h, m, 0).unwrap())
    }

    #[test]
    fn push_exchange_appends_user_mascot_pair() {
        let mut s = session_at(14, 30);
        s.push_exchange("こんにちは".to_string(), "やあ".to_string());
        assert_eq!(s.turns.len(), 2);
        assert_eq!(s.turns[0].role, Role::User);
        assert_eq!(s.turns[0].text, "こんにちは");
        assert_eq!(s.turns[1].role, Role::Mascot);
        assert_eq!(s.turns[1].text, "やあ");
    }

    #[test]
    fn chat_note_formats_title_and_body() {
        let mut s = session_at(14, 30);
        s.push_exchange("こんにちは".to_string(), "やあ".to_string());
        s.push_exchange("二言目".to_string(), "返答".to_string());
        let ended = chrono::Local
            .with_ymd_and_hms(2026, 8, 9, 14, 42, 0)
            .unwrap();
        let (title, body) = chat_note(&s, &ended);
        assert_eq!(title, "会話ログ 2026-08-09 14:30");
        assert_eq!(
            body,
            "**ユーザー**: こんにちは\n**miryam**: やあ\n\n\
             **ユーザー**: 二言目\n**miryam**: 返答\n\n\
             ---\n- 開始: 2026-08-09 14:30\n- 終了: 2026-08-09 14:42\nSource: miryam chat"
        );
    }

    #[test]
    fn chat_note_keeps_multiline_reply() {
        let mut s = session_at(9, 0);
        s.push_exchange("q".to_string(), "一行目\n二行目".to_string());
        let ended = chrono::Local.with_ymd_and_hms(2026, 8, 9, 9, 5, 0).unwrap();
        let (_, body) = chat_note(&s, &ended);
        assert!(body.starts_with("**ユーザー**: q\n**miryam**: 一行目\n二行目\n"));
    }

    #[test]
    fn with_mode_session_notes_mode_name_in_title() {
        let mode = ChatMode {
            name: "設計議論".to_string(),
            prompt: "p".to_string(),
            window: true,
        };
        let mut s = ChatSession::with_mode(
            chrono::Local
                .with_ymd_and_hms(2026, 8, 9, 14, 30, 0)
                .unwrap(),
            mode,
        );
        s.push_exchange("q".to_string(), "a".to_string());
        let ended = chrono::Local
            .with_ymd_and_hms(2026, 8, 9, 14, 42, 0)
            .unwrap();
        let (title, _) = chat_note(&s, &ended);
        assert_eq!(title, "会話ログ (設計議論) 2026-08-09 14:30");
    }

    #[test]
    fn split_choices_extracts_up_to_three() {
        let (body, choices) =
            split_choices("本文一行目。\n二行目。\n>> 候補A\n>> 候補B\n>> 候補C\n>> 候補D");
        assert_eq!(body, "本文一行目。\n二行目。");
        assert_eq!(choices, vec!["候補A", "候補B", "候補C"], "4 個目は捨てる");
    }

    #[test]
    fn split_choices_no_marker_returns_full_body() {
        let (body, choices) = split_choices("マーカーのない返答です。");
        assert_eq!(body, "マーカーのない返答です。");
        assert!(choices.is_empty());
    }

    #[test]
    fn split_choices_drops_empty_choice_and_keeps_midtext_marker() {
        let (body, choices) = split_choices("前半。\n>> 深掘りする\n後半。\n>>\n>>   ");
        assert_eq!(body, "前半。\n後半。", "本文途中のマーカー行も抽出される");
        assert_eq!(
            choices,
            vec!["深掘りする"],
            ">> のみ・空白のみの候補は捨てる"
        );
    }

    #[test]
    fn split_choices_tolerates_leading_whitespace_and_trims() {
        let (body, choices) = split_choices("本文。\n  >> 例文を 3 つ見せて  \n");
        assert_eq!(body, "本文。");
        assert_eq!(choices, vec!["例文を 3 つ見せて"]);
    }

    #[test]
    fn split_choices_all_markers_gives_empty_body() {
        let (body, choices) = split_choices(">> A\n>> B");
        assert_eq!(body, "");
        assert_eq!(choices, vec!["A", "B"]);
    }

    #[test]
    fn postprocess_window_keeps_multiline_and_trims() {
        assert_eq!(
            postprocess_window("  一行目\n\n二行目  \n").as_deref(),
            Some("一行目\n\n二行目")
        );
    }

    #[test]
    fn postprocess_window_caps_at_2000_chars() {
        let long = "あ".repeat(2500);
        assert_eq!(postprocess_window(&long).unwrap().chars().count(), 2000);
    }

    #[test]
    fn postprocess_window_empty_is_none() {
        assert!(postprocess_window("").is_none());
        assert!(postprocess_window("  \n \n").is_none());
    }

    #[test]
    fn modes_defaults_are_three_builtins() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        let modes = cfg.modes();
        let names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["雑談", "設計議論", "英語表現"]);
        assert!(!modes[0].window, "雑談は吹き出し");
        assert!(modes[1].window && modes[2].window, "議論系は会話窓");
        assert_eq!(modes[0].prompt, DEFAULT_CHAT_PERSONA);
    }

    #[test]
    fn modes_chat_prompt_overrides_casual_persona() {
        let cfg: ChatConfig = toml::from_str(r#"prompt = "関西弁で話して""#).unwrap();
        assert_eq!(cfg.modes()[0].prompt, "関西弁で話して");
    }

    #[test]
    fn modes_mode_entry_overrides_builtin_prompt_keeping_window() {
        let cfg: ChatConfig =
            toml::from_str("[[mode]]\nname = \"英語表現\"\nprompt = \"custom english\"").unwrap();
        let modes = cfg.modes();
        let english = modes.iter().find(|m| m.name == "英語表現").unwrap();
        assert_eq!(english.prompt, "custom english");
        assert!(english.window, "上書きでも表示先は変わらない");
        assert_eq!(modes.len(), 3, "上書きはモードを増やさない");
    }

    #[test]
    fn modes_casual_mode_entry_beats_chat_prompt() {
        let cfg: ChatConfig = toml::from_str(
            "prompt = \"chat prompt\"\n[[mode]]\nname = \"雑談\"\nprompt = \"mode prompt\"",
        )
        .unwrap();
        let casual = &cfg.modes()[0];
        assert_eq!(
            casual.prompt, "mode prompt",
            "[[chat.mode]] が [chat] prompt に勝つ"
        );
        assert!(!casual.window, "雑談は上書きしても吹き出しのまま");
    }

    #[test]
    fn modes_new_name_appends_window_mode() {
        let cfg: ChatConfig =
            toml::from_str("[[mode]]\nname = \"レビュー相談\"\nprompt = \"p\"").unwrap();
        let modes = cfg.modes();
        assert_eq!(modes.len(), 4);
        assert_eq!(modes[3].name, "レビュー相談");
        assert!(modes[3].window, "ユーザー定義モードは常に会話窓");
    }

    #[test]
    fn mode_validation_rejects_empty_and_duplicate_names() {
        let bad: ChatConfig = toml::from_str("[[mode]]\nname = \"  \"\nprompt = \"p\"").unwrap();
        assert!(bad.validate().is_err(), "空白のみの name は拒否");
        let bad: ChatConfig = toml::from_str(
            "[[mode]]\nname = \"A\"\nprompt = \"p\"\n[[mode]]\nname = \"A\"\nprompt = \"q\"",
        )
        .unwrap();
        assert!(bad.validate().is_err(), "toml 内の name 重複は拒否");
    }

    #[test]
    fn mode_rejects_unknown_keys_and_missing_prompt() {
        assert!(
            toml::from_str::<ChatConfig>("[[mode]]\nname = \"A\"\nprompt = \"p\"\nx = 1").is_err()
        );
        assert!(
            toml::from_str::<ChatConfig>("[[mode]]\nname = \"A\"").is_err(),
            "prompt は必須"
        );
    }

    #[test]
    fn build_mode_prompt_casual_equals_build_chat_prompt() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        let turns = vec![turn(Role::User, "一言目"), turn(Role::Mascot, "返答一")];
        let casual = &cfg.modes()[0];
        assert_eq!(
            build_mode_prompt(casual, &turns, "二言目", &test_snapshot()),
            build_chat_prompt(&cfg, &turns, "二言目", &test_snapshot()),
            "雑談モードのプロンプトは従来とバイト単位で一致する"
        );
    }

    #[test]
    fn build_mode_prompt_window_inserts_choices_instruction() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        let design = cfg
            .modes()
            .into_iter()
            .find(|m| m.name == "設計議論")
            .unwrap();
        let p = build_mode_prompt(&design, &[], "マイクロサービス分割の是非", &test_snapshot());
        assert!(p.starts_with(&design.prompt));
        assert!(
            p.contains("「>> 候補」の形式で 1 行 1 候補、最大 3 行"),
            "選択肢指示が入る"
        );
        assert!(p.contains("状況: 時間帯=morning"));
        assert!(p.ends_with("ユーザー: マイクロサービス分割の是非\nmiryam:"));
        let instr_pos = p.find(">> 候補").unwrap();
        let situation_pos = p.find("状況: ").unwrap();
        assert!(
            instr_pos < situation_pos,
            "指示はペルソナ直後・状況行より前"
        );
    }

    #[test]
    fn build_chat_prompt_has_no_choices_instruction() {
        let cfg: ChatConfig = toml::from_str("").unwrap();
        let p = build_chat_prompt(&cfg, &[], "やあ", &test_snapshot());
        assert!(
            !p.contains(">> 候補"),
            "雑談プロンプトにマーカー指示は入らない"
        );
    }
}
