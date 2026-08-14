//! 選択範囲に対して LLM にさせる操作。GTK には触らない。
//!
//! **操作を増やすときは `ACTIONS` に 1 エントリ足すだけ**で、ポップオーバー・保存・表示は
//! そのまま通る。プロンプトの組み立ては操作ごとの関数にしてあるので、文字列キーによる
//! 分岐を作らずに済み、それぞれ独立してテストできる。

use crate::reader::store::LlmQa;

/// プロンプトに載せる過去の問答の上限 (chat.rs が履歴 20 発言で切っているのに倣う)
pub const HISTORY_MAX: usize = 10;

/// 回答の文字数上限。吹き出しではなくメモ欄で読むので chat.rs の 300 字より緩い
pub const ANSWER_MAX_CHARS: usize = 2000;

/// 選択範囲に対して LLM にさせる操作
pub struct Action {
    /// サイドカーに保存する識別子 (`LlmQa::kind`)
    pub kind: &'static str,
    /// ポップオーバーに出す文言
    pub label: &'static str,
    /// ユーザーに質問文を打たせるか。false なら入力欄を出さず即実行する
    pub needs_question: bool,
    /// プロンプトの組み立て。`question` は `needs_question` が false のとき空
    pub prompt: fn(quote: &str, history: &[LlmQa], question: &str) -> String,
}

pub const ACTIONS: &[Action] = &[Action {
    kind: "ask",
    label: "LLM に聞く",
    needs_question: true,
    prompt: ask_prompt,
}];

pub fn find(kind: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.kind == kind)
}

/// 「聞く」のプロンプト。**引用文と過去の問答と今回の質問だけ**を渡す
/// (ページ全文や周辺文脈は渡さない — 送るものが画面で見えているものと一致するように)
fn ask_prompt(quote: &str, history: &[LlmQa], question: &str) -> String {
    let mut s = String::new();
    s.push_str("あなたは読書中の人の相棒です。以下は本文から抜き出された一節です。\n\n");
    s.push_str("---\n");
    s.push_str(quote);
    s.push_str("\n---\n\n");

    let recent = if history.len() > HISTORY_MAX {
        &history[history.len() - HISTORY_MAX..]
    } else {
        history
    };
    if !recent.is_empty() {
        s.push_str("この一節についてのこれまでのやりとり:\n");
        for qa in recent {
            s.push_str("Q: ");
            s.push_str(&qa.q);
            s.push_str("\nA: ");
            s.push_str(&qa.a);
            s.push('\n');
        }
        s.push('\n');
    }

    s.push_str("この一節について質問します。日本語で、簡潔に答えてください。\n");
    s.push_str("推測で補わず、一節から読み取れないことは読み取れないと言ってください。\n\n");
    s.push_str(question);
    s
}

/// LLM の出力を整える。前後の空白を落とし、長すぎる回答は切り詰める。
/// 空なら None (何も蓄積しない)
///
/// `chars().take(N)` は実際の文字数が N 以下ならそのまま全体を返す (飽和する) ため、
/// 「上限以下かどうかを先に判定してから分岐する」書き方は分岐そのものが冗長になる。
/// 判定に使う指標 (文字数かバイト数か) を変えても出力が変わらない等価な分岐を残さないよう、
/// 常に文字数で切り詰める 1 本の式にしてある。
pub fn postprocess(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(ANSWER_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qa(kind: &str, q: &str, a: &str) -> LlmQa {
        LlmQa { kind: kind.into(), q: q.into(), a: a.into(), at: chrono::Local::now() }
    }

    fn ask_action() -> &'static Action {
        find("ask").expect("ask がある")
    }

    #[test]
    fn find_returns_none_for_an_unknown_kind() {
        assert!(find("translate").is_none(), "まだ足していない操作は None");
        assert!(find("").is_none());
    }

    #[test]
    fn every_action_has_a_unique_kind_and_a_label() {
        let mut seen = std::collections::HashSet::new();
        for a in ACTIONS {
            assert!(!a.kind.is_empty(), "kind は空にできない");
            assert!(!a.label.is_empty(), "label は空にできない");
            assert!(seen.insert(a.kind), "kind が重複している: {}", a.kind);
        }
    }

    // 自分で考えた軸: `Action` のフィールド値そのもの (非空チェックだけでは、
    // 中身が食い違っていても素通りする)
    #[test]
    fn the_ask_action_has_the_expected_kind_label_and_needs_question() {
        let a = ask_action();
        assert_eq!(a.kind, "ask");
        assert_eq!(a.label, "LLM に聞く");
        assert!(a.needs_question, "「聞く」は質問文の入力を必要とする");
    }

    #[test]
    fn the_ask_prompt_carries_the_quote_and_the_question() {
        let p = (ask_action().prompt)("引用文です", &[], "これはどういう意味?");
        assert!(p.contains("引用文です"), "引用文が入る: {p}");
        assert!(p.contains("これはどういう意味?"), "質問が入る: {p}");
    }

    #[test]
    fn the_ask_prompt_carries_previous_exchanges() {
        let history = [qa("ask", "前の質問", "前の答え")];
        let p = (ask_action().prompt)("引用文", &history, "もっと簡単に言うと?");
        assert!(p.contains("前の質問"), "過去の質問が入る: {p}");
        assert!(p.contains("前の答え"), "過去の答えが入る: {p}");
    }

    // 自分で考えた軸: 質問と答えが `contains` だけでは区別できない形で
    // 入れ替わっていないか (ラベルと中身が正しく対応しているか)
    #[test]
    fn the_ask_prompt_labels_question_and_answer_correctly() {
        let history = [qa("ask", "前の質問", "前の答え")];
        let p = (ask_action().prompt)("引用文", &history, "いま");
        assert!(p.contains("Q: 前の質問"), "質問には Q: が付く: {p}");
        assert!(p.contains("A: 前の答え"), "答えには A: が付く: {p}");
    }

    #[test]
    fn the_ask_prompt_keeps_only_the_last_exchanges() {
        let history: Vec<LlmQa> = (0..HISTORY_MAX + 5)
            .map(|i| qa("ask", &format!("Q{i}"), &format!("A{i}")))
            .collect();
        let p = (ask_action().prompt)("引用文", &history, "いま");
        assert!(!p.contains("Q0"), "古いものは落ちる: {p}");
        let newest = HISTORY_MAX + 4;
        assert!(p.contains(&format!("Q{newest}")), "新しいものは残る: {p}");
    }

    #[test]
    fn the_ask_prompt_sends_nothing_but_the_quote_and_the_exchanges() {
        // 「送るものが画面で見えているものと一致する」— ページ全文や周辺文脈を混ぜないこと
        let p = (ask_action().prompt)("引用文", &[], "質問");
        assert!(!p.contains("ページ"), "ページ全文をほのめかす語が入らない: {p}");
    }

    #[test]
    fn postprocess_trims_and_keeps_multiple_lines() {
        assert_eq!(postprocess("  一行目\n二行目  \n"), Some("一行目\n二行目".to_string()));
    }

    #[test]
    fn postprocess_of_blank_output_is_none() {
        assert_eq!(postprocess("   \n\n "), None);
        assert_eq!(postprocess(""), None);
    }

    #[test]
    fn postprocess_caps_a_very_long_answer() {
        let long = "あ".repeat(ANSWER_MAX_CHARS + 100);
        let got = postprocess(&long).expect("空ではない");
        assert_eq!(got.chars().count(), ANSWER_MAX_CHARS, "文字数で切ること (バイトではない)");
    }

    #[test]
    fn answer_cap_is_2000() {
        assert_eq!(ANSWER_MAX_CHARS, 2000);
    }

    #[test]
    fn history_cap_is_10() {
        assert_eq!(HISTORY_MAX, 10);
    }
}
