//! リーダー UI の「判断を担う述語」を GTK から切り離して置くところ。
//!
//! ここに置いてあるのは、どれも元は GTK のウィジェットを組み立てるコードの中に
//! 埋まっていた小さな規則 (次の一致はどれか、状態表示は何と出すか、Enter は
//! 検索し直しか次へ送るか、絞り込みの選択肢の何番目を選ぶか) で、GTK に触れる
//! 位置にあると人も自動テストも触れない。**判断はここに置き、GTK 側は結果を
//! ウィジェットに流すだけにする。**
//!
//! この module は GTK に依存しない (依存させないこと)。

/// 何も検索していないときの案内。`status_text` が返す文言でもあり、
/// 検索タブを作った直後のラベルの初期値でもある
pub const STATUS_IDLE: &str = "語を入れて Enter で検索します (Ctrl+F)";

/// 一致の一覧のうち、次に選ぶものの位置を返す。
///
/// - `len == 0` (一致が無い) なら移動先は無い (`None`)
/// - まだどこも選んでいない (`current == None`) なら、進むとき (`delta > 0`) は
///   先頭から、それ以外は末尾から始める
/// - 端は巻き戻る (末尾の次は先頭、先頭の前は末尾)
///
/// `delta` は実際には ±1 しか渡ってこないが、規則自体はどの幅でも同じに書ける
pub fn advance(current: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let index = match current {
        None if delta > 0 => 0,
        None => len - 1,
        // `rem_euclid` は負でも 0..len に収まるので、はみ出しで落ちない
        Some(i) => (i as isize + delta).rem_euclid(len as isize) as usize,
    };
    Some(index)
}

/// 検索タブの状態表示。判断の順番そのものが仕様:
///
/// 1. 走査中なら、途中経過の件数を出す (まだ増える)
/// 2. 一度も検索していないなら案内を出す
/// 3. 検索した結果 1 ページも見つからなかったなら、そう言う
/// 4. それ以外は件数とページ数を出す
///
/// `pages` は一致のあったページ数 (= 一覧の行数)、`marks` は一致そのものの総数。
/// 走査中は 2. 〜 4. より優先する: 走査の途中で「見つかりませんでした」と
/// 出してしまうと、まだ探している最中なのに無いと言うことになる
///
/// **呼び出し側の不変条件:** `pages` と `marks` を数えているのはどちらも
/// `SearchTab` (`hits` の要素数と `total_marks`) で、一致が空のページは積まれない。
/// これを守っているのは `SearchTab::push` 冒頭の `rects.is_empty()` の門番
/// (と、そこを呼ぶ `install_search` 側の `if !rects.is_empty()` フィルタ) であって、
/// `Search::push_hits` **ではない** (あちらが守っているのは本文の強調に使う
/// 別のベクタ `Search::hits`)。この門番がある限り常に `marks >= pages` で、
/// `pages == 0` と `marks == 0` は必ず同時に成り立つので、3. の判定をどちらで
/// 書いても実際の表示は変わらない (テストでも区別できない)。「1 件も一致する
/// ページが無かった」という意味に近い `pages` で書いてある
pub fn status_text(running: bool, searched: bool, pages: usize, marks: usize) -> String {
    if running {
        format!("検索中… ({marks} 件)")
    } else if !searched {
        STATUS_IDLE.to_string()
    } else if pages == 0 {
        "見つかりませんでした".to_string()
    } else {
        format!("{marks} 件見つかりました ({pages} ページ)")
    }
}

/// 検索欄で Enter を押したとき、走査をやり直すか (`true`) 次の一致へ送るか (`false`)。
///
/// `last` は直近で走査を始めた語 (`None` は「まだ一度も走査していない」)。
/// 語が変わっていれば走査し直し、同じままなら次の一致へ送る。
/// まだ一度も走査していなければ、空の語であっても走査側へ渡す
/// (`SearchTab::start` が空の語を「一覧を空にするだけ」として扱う)
pub fn should_restart(last: Option<&str>, query: &str) -> bool {
    last != Some(query)
}

/// タグ絞り込みドロップダウンで選ぶべき位置。先頭は常に「すべて」なので、
/// タグ `all_tags[i]` は `i + 1` 番目にある。
///
/// 絞り込み無し (`None`)、および `all_tags` に無いタグを渡されたときは
/// 「すべて」(0) を返す。後者は本来起こらない (呼び出し元が `all_tags` に
/// 残っていない絞り込みを先に落としている) が、選択肢に無い値を選ぼうとして
/// 黙って別の項目を選んでしまうよりは「すべて」に倒すほうが安全
pub fn selected_index(all_tags: &[String], filter: Option<&str>) -> u32 {
    let Some(tag) = filter else {
        return 0;
    };
    all_tags.iter().position(|t| t == tag).map_or(0, |i| (i + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_without_hits_goes_nowhere() {
        assert_eq!(advance(None, 0, 1), None);
        assert_eq!(advance(None, 0, -1), None);
        assert_eq!(advance(Some(0), 0, 1), None, "一覧を捨てた直後でも落ちない");
    }

    #[test]
    fn advance_from_nothing_starts_at_the_first_when_going_forward() {
        assert_eq!(advance(None, 3, 1), Some(0));
    }

    #[test]
    fn advance_from_nothing_starts_at_the_last_when_going_backward() {
        assert_eq!(advance(None, 3, -1), Some(2));
    }

    #[test]
    fn advance_steps_one_at_a_time() {
        assert_eq!(advance(Some(0), 3, 1), Some(1));
        assert_eq!(advance(Some(1), 3, 1), Some(2));
        assert_eq!(advance(Some(2), 3, -1), Some(1));
    }

    #[test]
    fn advance_wraps_around_at_both_ends() {
        assert_eq!(advance(Some(2), 3, 1), Some(0), "末尾の次は先頭");
        assert_eq!(advance(Some(0), 3, -1), Some(2), "先頭の前は末尾");
    }

    /// 一致が 1 件だけなら、進んでも戻っても同じところに留まる
    #[test]
    fn advance_with_a_single_hit_stays_put() {
        assert_eq!(advance(Some(0), 1, 1), Some(0));
        assert_eq!(advance(Some(0), 1, -1), Some(0));
        assert_eq!(advance(None, 1, 1), Some(0));
        assert_eq!(advance(None, 1, -1), Some(0));
    }

    /// 一覧が縮んだあとの古い位置でも 0..len に収まる (はみ出したインデックスを
    /// そのまま返さない)。`rem_euclid` を `%` や単純な `+1` にする変異を捕まえる
    #[test]
    fn advance_keeps_a_stale_index_inside_the_list() {
        let next = advance(Some(9), 3, 1).expect("一致があるので進める");
        assert!(next < 3, "はみ出した位置から進んでも一覧の中に収まること: {next}");
        let prev = advance(Some(9), 3, -1).expect("一致があるので戻れる");
        assert!(prev < 3, "戻る側も同じ: {prev}");
    }

    #[test]
    fn status_while_running_shows_the_running_total() {
        assert_eq!(status_text(true, true, 2, 5), "検索中… (5 件)");
    }

    /// 走査中は「まだ検索していない」「見つかりませんでした」より優先する。
    /// 順番を入れ替える変異 (running の判定を後ろに回す) を捕まえる
    #[test]
    fn running_wins_over_every_other_message() {
        assert_eq!(status_text(true, false, 0, 0), "検索中… (0 件)", "始めた直後もまず検索中");
        assert_eq!(
            status_text(true, true, 0, 0),
            "検索中… (0 件)",
            "まだ 1 件も無くても、探している間は「見つかりませんでした」と言わない"
        );
    }

    #[test]
    fn status_before_any_search_is_the_idle_message() {
        assert_eq!(status_text(false, false, 0, 0), STATUS_IDLE);
    }

    #[test]
    fn status_after_a_fruitless_search_says_so() {
        assert_eq!(status_text(false, true, 0, 0), "見つかりませんでした");
    }

    #[test]
    fn status_after_a_search_counts_marks_and_pages() {
        assert_eq!(status_text(false, true, 2, 5), "5 件見つかりました (2 ページ)");
    }

    /// 件数とページ数は別物 (同じ値を 2 回出す変異を捕まえる)
    #[test]
    fn status_does_not_confuse_marks_with_pages() {
        let text = status_text(false, true, 1, 7);
        assert_eq!(text, "7 件見つかりました (1 ページ)");
    }

    #[test]
    fn the_first_enter_starts_a_search() {
        assert!(should_restart(None, "重要"));
    }

    #[test]
    fn the_same_query_moves_on_instead_of_searching_again() {
        assert!(!should_restart(Some("重要"), "重要"));
    }

    #[test]
    fn a_changed_query_starts_over() {
        assert!(should_restart(Some("重要"), "重要な"));
        assert!(should_restart(Some("重要"), ""), "空にしたのも別の語");
    }

    /// 大文字小文字や前後の空白の違いも「別の語」として扱う (検索そのものは
    /// 大文字小文字を区別しないが、ここは「同じ語のまま Enter を押したか」を
    /// 見ているだけなので、文字列として違えば検索し直す)
    #[test]
    fn should_restart_compares_the_text_as_typed() {
        assert!(should_restart(Some("abc"), "ABC"));
        assert!(should_restart(Some("abc"), "abc "));
    }

    /// 「まだ検索していない」と「空の語で検索した」は別物。前者では空のまま
    /// Enter を押しても走査側へ渡す (`last` を `Option` にした意味がここにある)
    #[test]
    fn never_searched_differs_from_having_searched_for_nothing() {
        assert!(should_restart(None, ""));
        assert!(!should_restart(Some(""), ""));
    }

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_filter_selects_the_first_item() {
        assert_eq!(selected_index(&tags(&["a", "b"]), None), 0);
    }

    #[test]
    fn a_filter_selects_its_tag_one_below_the_all_item() {
        let all = tags(&["a", "b", "c"]);
        assert_eq!(selected_index(&all, Some("a")), 1);
        assert_eq!(selected_index(&all, Some("b")), 2);
        assert_eq!(selected_index(&all, Some("c")), 3);
    }

    #[test]
    fn an_unknown_filter_falls_back_to_the_all_item() {
        assert_eq!(selected_index(&tags(&["a"]), Some("b")), 0);
        assert_eq!(selected_index(&[], Some("a")), 0);
    }

    /// 「すべて」の分の 1 つずらしを落とす変異 (`position` の結果をそのまま返す)
    /// を捕まえる。先頭のタグは 0 ではなく 1
    #[test]
    fn the_first_tag_is_never_at_index_zero() {
        assert_ne!(selected_index(&tags(&["a", "b"]), Some("a")), 0);
    }

    /// タグの一致は完全一致 (大文字小文字も区別する)。`matches_filter` と同じ扱い
    #[test]
    fn selected_index_is_case_sensitive() {
        assert_eq!(selected_index(&tags(&["A"]), Some("a")), 0, "違うタグなので「すべて」");
    }

    /// 同じ名前のタグが二重に並ぶことは `collect_tags` の重複排除により無いが、
    /// 万一並んでも先に出てきたほうを選ぶ (後ろを選んで別の行を絞り込まない)
    #[test]
    fn selected_index_picks_the_first_of_duplicates() {
        assert_eq!(selected_index(&tags(&["a", "b", "a"]), Some("a")), 1);
    }
}
