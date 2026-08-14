//! 抄訳と感想台詞のプロンプト構築と出力分割。GTK には触らない。
//!
//! 書き出し 1 回につき LLM 呼び出しは 1 回だけ (仕様書)。プロンプトには
//! **目次 + ハイライト (引用・メモ・タグ)** を渡し、出力は `---` 区切りで
//! 「1 行目以降が抄訳本文 / その後が感想台詞 (1 行 1 本)」に分割する。

use crate::reader::export::Section;
use crate::reader::store::Highlight;

/// 感想台詞の上限 (仕様書の 3〜5 本の上限側)
pub const REMARKS_MAX: usize = 5;

/// 抄訳 + 感想台詞のプロンプト。目次が無ければページ番号で代用する (仕様書)
pub fn build_prompt(sections: &[Section], highlights: &[Highlight]) -> String {
    let mut s = String::new();
    s.push_str("あなたは読書中の人の相棒です。以下は読んだ本の目次と、その本に引いたマーカー一覧です。\n\n");
    s.push_str("## 目次\n");
    if sections.is_empty() {
        s.push_str("(この本は目次を持ちません)\n");
    } else {
        for sec in sections {
            s.push_str(&format!("- {} (p.{})\n", sec.title, sec.page + 1));
        }
    }
    s.push_str("\n## マーカー\n");
    let mut sorted: Vec<&Highlight> = highlights.iter().collect();
    sorted.sort_by_key(|h| h.page);
    for h in sorted {
        s.push_str(&format!("p.{} 引用: {}\n", h.page + 1, h.quote));
        if !h.memo.trim().is_empty() {
            s.push_str(&format!("メモ: {}\n", h.memo));
        }
        if !h.tags.is_empty() {
            s.push_str(&format!("タグ: {}\n", h.tags.join(", ")));
        }
    }
    s.push_str(
        "\nこの本を「目次の構造に沿って、引いた箇所を軸にした要約」にしてください。\
         要約は Markdown の段落で。その後、--- を 1 行置いて、\
         あなた自身の感想の台詞を 1 行 1 本で 3〜5 本書いてください。\
         感想台詞は 40 文字以内の日本語で、語尾は「〜」にしないでください。\n",
    );
    s
}

/// LLM 出力を `---` 区切りで (抄訳本文, 感想台詞) に分割する。
/// 区切りは**単独の行** (`---` ちょうど) だけを認める — 本文中に `---` を含む行が
/// あっても区切りと解釈しない。区切りが無い・本文が空は None (形式違反 — 書き出さない)。
/// 感想台詞は `REMARKS_MAX` 本まで。0 本でも本文があれば Some (感想は無くても書き出しはする)
pub fn split(stdout: &str) -> Option<(String, Vec<String>)> {
    let sep = stdout.lines().position(|l| l.trim() == "---")?;
    let body: String = stdout
        .lines()
        .take(sep)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if body.is_empty() {
        return None;
    }
    let remarks: Vec<String> = stdout
        .lines()
        .skip(sep + 1)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(REMARKS_MAX)
        .map(str::to_string)
        .collect();
    Some((body, remarks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::export::Section;
    use crate::reader::store::Highlight;

    fn hl(page: usize, quote: &str, memo: &str, tags: &[&str]) -> Highlight {
        Highlight {
            id: "x".into(),
            page,
            color: "yellow".into(),
            rects: vec![[0.0, 0.0, 0.1, 0.1]],
            quote: quote.into(),
            memo: memo.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            llm: vec![],
            created_at: chrono::Local::now(),
        }
    }

    fn sec(title: &str, page: usize) -> Section {
        Section { title: title.into(), page }
    }

    #[test]
    fn build_prompt_carries_sections_and_highlights() {
        let sections = vec![sec("第2章 設計", 5)];
        let hs = vec![hl(5, "引用文です", "メモです", &["重要"])];
        let p = build_prompt(&sections, &hs);
        assert!(p.contains("第2章 設計"), "目次が入る: {p}");
        assert!(p.contains("引用文です"), "引用が入る: {p}");
        assert!(p.contains("メモです"), "メモが入る: {p}");
        assert!(p.contains("重要"), "タグが入る: {p}");
    }

    #[test]
    fn build_prompt_without_outline_uses_pages_as_placeholders() {
        let hs = vec![hl(0, "引用", "", &[])];
        let p = build_prompt(&[], &hs);
        assert!(p.contains("p.1"), "目次が無いならページ番号で代用: {p}");
    }

    #[test]
    fn build_prompt_asks_for_summary_and_remarks_format() {
        let p = build_prompt(&[], &[hl(0, "q", "", &[])]);
        assert!(p.contains("---"), "区切り記号を指定する: {p}");
        assert!(p.contains("感想"), "感想台詞を要求する: {p}");
    }

    #[test]
    fn split_returns_body_and_remarks() {
        let (body, remarks) =
            split("本文一行目\n本文二行目\n---\n感想1\n感想2\n感想3").expect("分割できる");
        assert_eq!(body, "本文一行目\n本文二行目");
        assert_eq!(remarks, vec!["感想1", "感想2", "感想3"]);
    }

    #[test]
    fn split_trims_and_drops_empty_remark_lines() {
        // 区切り行自体も ` --- ` のように空白が前後してもよい (変異検査のため
        // ここに入れておく — `.trim()` を外すとこの行が区切りとして見えなくなる)
        let (_, remarks) =
            split("本文\n --- \n\n  感想1  \n\n感想2\n").expect("分割できる");
        assert_eq!(remarks, vec!["感想1", "感想2"], "前後の空白を落とし空行を捨てる");
    }

    #[test]
    fn split_caps_remarks_at_max() {
        let lines: Vec<String> = (0..=REMARKS_MAX).map(|i| format!("感想{i}")).collect();
        let (_, remarks) = split(&format!("本文\n---\n{}", lines.join("\n"))).expect("分割できる");
        assert_eq!(remarks.len(), REMARKS_MAX, "5 本まで (仕様 3〜5 本)");
    }

    #[test]
    fn split_without_separator_is_none() {
        // LLM が --- を忘れた = 形式違反。書き出してはいけない
        assert_eq!(split("本文だけ"), None);
    }

    #[test]
    fn split_requires_a_standalone_separator_line() {
        // 本文中に --- を含む行があっても区切りと解釈しない
        assert_eq!(split("本文は x---y です"), None);
        // 区切りが先頭行に無い形でも、本文行の中の --- は区切りと解釈しない
        // (contains で誤認すると本文が生まれて Some になる — 変異検査のため)
        assert_eq!(split("本文の一行目\nx---y は本文の続き"), None);
    }

    #[test]
    fn split_of_blank_output_is_none() {
        assert_eq!(split(""), None);
        assert_eq!(split("   \n\n"), None);
        // 区切りはあるが本文が空 = 形式違反 (body が空のとき None にする判定を固定)
        assert_eq!(split("---\n感想"), None);
    }

    #[test]
    fn split_with_zero_remarks_keeps_the_body() {
        let (body, remarks) = split("本文\n---").expect("分割できる");
        assert_eq!(body, "本文");
        assert!(remarks.is_empty(), "感想 0 本でも本文があれば進める");
    }

    // 自分で考えた軸: プロンプトのハイライトはページ順に並ぶ (実装が sort していることの固定)
    #[test]
    fn build_prompt_sorts_highlights_by_page() {
        let hs = vec![hl(5, "後ろのページ", "", &[]), hl(0, "先頭のページ", "", &[])];
        let p = build_prompt(&[], &hs);
        assert!(
            p.find("先頭のページ").unwrap() < p.find("後ろのページ").unwrap(),
            "ページ順に並ぶ: {p}"
        );
    }

    // 自分で考えた軸: メモが空白だけ・タグが空ならプロンプトに出さない
    // (実装の `trim().is_empty()` / `is_empty()` 条件を固定する)
    #[test]
    fn build_prompt_omits_a_blank_memo_and_empty_tags() {
        let hs = vec![hl(0, "引用", "   ", &[])];
        let p = build_prompt(&[], &hs);
        assert!(!p.contains("メモ: "), "空白だけのメモは出さない: {p}");
        assert!(!p.contains("タグ: "), "タグが空なら出さない: {p}");
    }

    #[test]
    fn remarks_cap_is_5() {
        assert_eq!(REMARKS_MAX, 5);
    }

    /// 境界: ちょうど REMARKS_MAX 本は全部残り、1 本多いと最後が落ちる
    /// (MAX+1 の飛び越えだけでは off-by-one が見えない)
    #[test]
    fn split_keeps_exactly_remarks_max_remarks() {
        let at_cap: Vec<String> = (0..REMARKS_MAX).map(|i| format!("感想{i}")).collect();
        let (_, got) = split(&format!("本文\n---\n{}", at_cap.join("\n"))).expect("分割できる");
        assert_eq!(got, at_cap, "ちょうど上限は全部残る");

        let over: Vec<String> = (0..=REMARKS_MAX).map(|i| format!("感想{i}")).collect();
        let (_, got) = split(&format!("本文\n---\n{}", over.join("\n"))).expect("分割できる");
        assert_eq!(got, at_cap, "1 本多いと最後が落ちる");
    }

    // 自分で考えた軸: 本文の前後の空白行も落とす (書き出す本文に余計な空行を残さない。
    // body 側の `.trim()` を外す変異を捕まえる)
    #[test]
    fn split_trims_the_body() {
        let (body, _) = split("\n 本文 \n\n---\n感想").expect("分割できる");
        assert_eq!(body, "本文");
    }

    /// 境界: `---` が 2 行あるなら**最初**が区切り (本文が `---` を
    /// テーマ区切りに使っていても、最初の 1 つ目が区切り)。
    /// `position` を `rposition` に変える変異を捕まえる。
    /// 2 行目の `---` は感想側の 1 本としてそのまま残る (感想は整形しない)
    #[test]
    fn split_uses_the_first_separator_line() {
        let (body, remarks) = split("本文\n---\n中間\n---\n感想").expect("分割できる");
        assert_eq!(body, "本文");
        assert_eq!(remarks, vec!["中間", "---", "感想"], "最初の --- より後が全部感想側");
    }
}
