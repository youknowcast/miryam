//! 注釈を Inkdrop ノート用の Markdown にし、作成/更新のペイロードを組む。GTK には触らない。
//!
//! ノート 1 件 = PDF 1 冊 (仕様書「Inkdrop ノート (1 PDF = 1 ノートを更新)」)。
//! 本文は `export::compose_body` が「抄訳 + ハイライト節 + 出所フッタ」に組み立てる。

use crate::reader::outline::OutlineItem;
use crate::reader::store::Highlight;

/// 平坦化した目次項目。書き出しは階層を持たない — 見出し文字列とページ番号だけ
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    /// 0 始まり
    pub page: usize,
}

/// 目次ツリーをページ順の平坦リストにする。**ページを持たない項目は落とす**
/// (飛び先が無い見出しは書き出しの見出しとして意味をなさない)
pub fn sections(outline: &[OutlineItem]) -> Vec<Section> {
    fn walk(items: &[OutlineItem], out: &mut Vec<Section>) {
        for item in items {
            if let Some(page) = item.page {
                out.push(Section { title: item.title.clone(), page });
            }
            walk(&item.children, out);
        }
    }
    let mut out = Vec::new();
    walk(outline, &mut out);
    out.sort_by_key(|s| s.page);
    out
}

/// そのページが属する見出し (ページ以下の直近の目次項目)。目次が無い・最初の見出しより
/// 前なら None (そのハイライトは見出し無しで並べる)
pub fn heading_for(sections: &[Section], page: usize) -> Option<&str> {
    sections.iter().rev().find(|s| s.page <= page).map(|s| s.title.as_str())
}

/// 注釈 → 「ハイライト」節の Markdown。ページ順 (同ページは作った順 = 安定ソート)。
/// 目次があれば `### 見出し` で区切り、無ければフラットに並べる。
/// 引用は各行を `> ` にし、行頭の `>` は `\>` にエスケープする (ネスト引用と解釈されないように)
pub fn highlights_markdown(highlights: &[Highlight], sections: &[Section]) -> String {
    let mut sorted: Vec<&Highlight> = highlights.iter().collect();
    sorted.sort_by_key(|h| h.page);

    let mut out = String::new();
    let mut current_heading: Option<String> = None;
    for h in sorted {
        let heading = heading_for(sections, h.page).map(str::to_string);
        if heading != current_heading {
            if let Some(h) = &heading {
                out.push_str("### ");
                out.push_str(h);
                out.push('\n');
            }
            current_heading = heading;
        }
        out.push_str(&format!("p.{}\n", h.page + 1));
        for line in h.quote.lines() {
            let escaped =
                if line.starts_with('>') { format!("\\{line}") } else { line.to_string() };
            out.push_str("> ");
            out.push_str(&escaped);
            out.push('\n');
        }
        if !h.memo.trim().is_empty() {
            out.push_str(&h.memo);
            out.push('\n');
        }
        if !h.tags.is_empty() {
            let tags: Vec<String> = h.tags.iter().map(|t| format!("`#{t}`")).collect();
            out.push_str(&tags.join(" "));
            out.push('\n');
        }
        out.push('\n');
    }
    out.trim_end_matches('\n').to_string()
}

/// ノートのタイトル。PDF のメタデータ title、無ければファイル名 (仕様書)
pub fn title_for(doc_title: Option<&str>, file_name: &str) -> String {
    doc_title.map(str::to_string).unwrap_or_else(|| file_name.to_string())
}

/// ノート本文: 抄訳 + ハイライト節 + 出所フッタ (既存 `capture_note` と同じ流儀)
pub fn compose_body(digest_body: &str, highlights_md: &str) -> String {
    let date = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    format!(
        "## 抄訳\n\n{digest_body}\n\n## ハイライト\n\n{highlights_md}\n\nSource: miryam-reader\nUpdated: {date}"
    )
}

/// POST /notes のボディ (既存 `note_payload` と同じフィールド。reader 用に再公開するのは
/// 更新ペイロードと対でこのモジュールに揃えるため)
pub fn create_payload(book_id: &str, title: &str, body: &str) -> String {
    serde_json::json!({
        "doctype": "markdown",
        "bookId": book_id,
        "status": "none",
        "share": "private",
        "title": title,
        "body": body,
    })
    .to_string()
}

/// PUT /notes/<id> のボディ。Inkdrop の更新は `_id` と `_rev` が要る (仕様書)
pub fn update_payload(book_id: &str, id: &str, rev: &str, title: &str, body: &str) -> String {
    serde_json::json!({
        "_id": id,
        "_rev": rev,
        "doctype": "markdown",
        "bookId": book_id,
        "status": "none",
        "share": "private",
        "title": title,
        "body": body,
    })
    .to_string()
}

/// GET /notes/<id> の応答から `_rev` を取る。無ければ None (更新を諦めて失敗扱い)
pub fn rev_from(note_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(note_json)
        .ok()?
        .get("_rev")?
        .as_str()
        .map(str::to_string)
}

/// POST /notes の応答から `_id` を取る (次回の更新に `note_id` として覚える)
pub fn id_from(note_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(note_json)
        .ok()?
        .get("_id")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::outline::OutlineItem;
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

    fn sec(title: &str, page: Option<usize>, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem { title: title.into(), page, children }
    }

    #[test]
    fn sections_flatten_the_tree_in_page_order() {
        let outline = vec![
            sec("第1章", Some(0), vec![sec("1.1", Some(0), vec![]), sec("1.2", Some(2), vec![])]),
            sec("第2章", Some(4), vec![]),
        ];
        let got: Vec<(String, usize)> =
            sections(&outline).into_iter().map(|s| (s.title, s.page)).collect();
        assert_eq!(got, vec![
            ("第1章".into(), 0),
            ("1.1".into(), 0),
            ("1.2".into(), 2),
            ("第2章".into(), 4),
        ]);
    }

    #[test]
    fn sections_skip_items_without_a_page() {
        let outline = vec![sec("無ページ", None, vec![sec("子", Some(1), vec![])])];
        let got: Vec<(String, usize)> =
            sections(&outline).into_iter().map(|s| (s.title, s.page)).collect();
        assert_eq!(got, vec![("子".into(), 1)], "ページを持たない項目は落とす");
    }

    #[test]
    fn sections_sort_by_page_even_when_the_tree_is_out_of_order() {
        // ツリー順 = ページ順とは限らない (PDF の目次は必ずしも昇順ではない)
        let outline = vec![
            sec("第3章", Some(4), vec![]),
            sec("第1章", Some(0), vec![]),
            sec("第2章", Some(2), vec![]),
        ];
        let got: Vec<(String, usize)> =
            sections(&outline).into_iter().map(|s| (s.title, s.page)).collect();
        assert_eq!(got, vec![
            ("第1章".into(), 0),
            ("第2章".into(), 2),
            ("第3章".into(), 4),
        ], "ツリー順でなくページ順に並ぶ");
    }

    #[test]
    fn heading_for_picks_the_nearest_preceding_section() {
        let secs = sections(&[sec("第1章", Some(0), vec![]), sec("第2章", Some(5), vec![])]);
        assert_eq!(heading_for(&secs, 0), Some("第1章"));
        assert_eq!(heading_for(&secs, 3), Some("第1章"), "章と章の間は前の章");
        assert_eq!(heading_for(&secs, 5), Some("第2章"));
        assert_eq!(heading_for(&secs, 99), Some("第2章"), "末尾を超えても最後の章");
    }

    #[test]
    fn heading_for_returns_none_without_sections_or_before_the_first() {
        assert_eq!(heading_for(&[], 3), None, "目次が無い");
        let secs = sections(&[sec("第2章", Some(5), vec![])]);
        assert_eq!(heading_for(&secs, 2), None, "最初の見出しより前");
    }

    #[test]
    fn highlights_markdown_orders_by_page_and_groups_headings() {
        let hs = vec![
            hl(5, "五ページの引用", "五のメモ", &["重要"]),
            hl(0, "先頭の引用", "", &[]),
            hl(0, "同じページの二つ目", "", &[]),
        ];
        let secs = sections(&[sec("第1章", Some(0), vec![]), sec("第2章", Some(5), vec![])]);
        let md = highlights_markdown(&hs, &secs);
        assert!(
            md.find("先頭の引用").unwrap() < md.find("同じページの二つ目").unwrap(),
            "同ページは作った順 (安定)"
        );
        assert!(
            md.find("同じページの二つ目").unwrap() < md.find("五ページの引用").unwrap(),
            "ページ順に並ぶ"
        );
        assert!(md.contains("### 第1章"));
        assert!(md.contains("### 第2章"));
        assert_eq!(md.matches("### 第1章").count(), 1, "見出しは同ページの複数ハイライトでも 1 回だけ");
        assert_eq!(md.matches("### 第2章").count(), 1);
        assert!(md.contains("p.6"), "p.N は 1 始まり");
    }

    #[test]
    fn highlights_markdown_ends_without_a_trailing_blank_line() {
        let hs = vec![hl(0, "引用", "メモ", &["タグ"])];
        let md = highlights_markdown(&hs, &[]);
        assert!(!md.ends_with('\n'), "末尾に余計な空行を出さない: {md:?}");
    }

    #[test]
    fn highlights_markdown_without_outline_is_flat() {
        let hs = vec![hl(1, "引用", "", &[])];
        let md = highlights_markdown(&hs, &[]);
        assert!(!md.contains("###"), "目次が無ければ見出しを出さない: {md}");
        assert!(md.contains("p.2"));
        assert!(md.contains("> 引用"));
    }

    #[test]
    fn highlights_markdown_escapes_a_leading_gt_in_a_quote() {
        // "> は引用" の引用文は、そのまま行頭に置くとネストした引用と解釈される
        let hs = vec![hl(0, "> は引用\n二行目", "", &[])];
        let md = highlights_markdown(&hs, &[]);
        assert!(md.contains("> \\> は引用"), "先頭の > はエスケープ: {md}");
        assert!(md.contains("> 二行目"), "2 行目以降は普通に引用行へ: {md}");
    }

    #[test]
    fn highlights_markdown_skips_a_whitespace_only_memo() {
        let hs = vec![hl(0, "引用", "   ", &[])];
        let md = highlights_markdown(&hs, &[]);
        assert_eq!(md, "p.1\n> 引用", "空白だけのメモは出さない: {md:?}");
    }

    #[test]
    fn highlights_markdown_shows_memo_and_tags() {
        let hs = vec![hl(0, "引用", "長いメモです", &["重要", "あとで"])];
        let md = highlights_markdown(&hs, &[]);
        assert!(md.contains("長いメモです"));
        assert!(md.contains("`#重要`"), "タグはインラインコードで: {md}");
        assert!(md.contains("`#あとで`"));
    }

    #[test]
    fn title_for_uses_doc_title_and_falls_back_to_file_name() {
        assert_eq!(title_for(Some("設計メモ"), "foo.pdf"), "設計メモ");
        assert_eq!(title_for(None, "foo.pdf"), "foo.pdf");
    }

    #[test]
    fn compose_body_has_summary_highlights_and_source_footer() {
        let body = compose_body("抄訳本文", "ハイライト節");
        assert!(body.starts_with("## 抄訳\n\n抄訳本文\n\n## ハイライト\n\nハイライト節"));
        assert!(body.contains("Source: miryam-reader"), "出所フッタ: {body}");
        assert!(body.contains("Updated: "), "更新日時: {body}");
    }

    #[test]
    fn create_payload_has_the_note_fields() {
        let v: serde_json::Value =
            serde_json::from_str(&create_payload("book:r", "題", "本文")).unwrap();
        assert_eq!(v["doctype"], "markdown");
        assert_eq!(v["bookId"], "book:r");
        assert_eq!(v["title"], "題");
        assert_eq!(v["body"], "本文");
    }

    #[test]
    fn update_payload_carries_id_and_rev() {
        let v: serde_json::Value =
            serde_json::from_str(&update_payload("book:r", "note:x", "1-abc", "題", "本文"))
                .unwrap();
        assert_eq!(v["_id"], "note:x");
        assert_eq!(v["_rev"], "1-abc");
        assert_eq!(v["bookId"], "book:r");
    }

    #[test]
    fn rev_from_parses_the_response() {
        assert_eq!(
            rev_from(r#"{"_id":"note:x","_rev":"1-abc"}"#).as_deref(),
            Some("1-abc")
        );
        assert_eq!(rev_from("not json"), None);
        assert_eq!(rev_from(r#"{"_id":"note:x"}"#), None, "_rev が無い");
    }

    #[test]
    fn id_from_parses_the_response() {
        assert_eq!(
            id_from(r#"{"_id":"note:x","_rev":"1-abc"}"#).as_deref(),
            Some("note:x")
        );
        assert_eq!(id_from("not json"), None);
    }
}
