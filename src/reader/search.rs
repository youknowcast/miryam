//! 全文検索の進行状態。GTK には触らない。
//!
//! 検索はメインループを止めないよう idle で少しずつ進める。ここはその
//! 「どこまで進んだか」「何が見つかったか」だけを持つ。

/// 1 回の idle で走査するページ数。
///
/// **実測値 (release/debug とも同程度、poppler の C 側が支配的なので差が出ない):**
/// groff で作った文字の詰まった 366 ページの PDF (`.ll 6i` 幅、1 ページ 40 行超) を
/// `find_hits` (正規化込み) で全ページ走査したところ、1 ページあたり概ね 0.7〜0.8ms。
/// 元の 32 ページ単位では 1 チャンクあたり平均 21〜33ms・最大 35ms となり、8ms の目安を
/// 大きく超えていた。実機で `install_search` の idle クロージャそのものを **32 ページの
/// 設定で** 計測しても同じ桁 (20〜35ms) だったので、この end-to-end 計測が裏付けたのは
/// 「UI 側の追加コスト (`tab.push` の行構築・`set_search_hits` の比較・`queue_draw`) は
/// 無視できる程度で find_text 自体が支配的」ということであって、6 という値そのものを
/// end-to-end で取り直したわけではない (6 では idle クロージャの再計測をしていない)。
/// 6 は 1 ページあたりの実測レートからの見積もり (0.7〜0.8ms × 6 ≒ 4.2〜4.8ms) で、
/// 8ms の目安に十分収まるとして採用した
pub const CHUNK_PAGES: usize = 6;

/// 1 ページ分の一致。`rects` は正規化座標 (左上原点、0.0〜1.0)
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub rects: Vec<[f64; 4]>,
}

#[derive(Debug)]
pub struct Search {
    pub query: String,
    next_page: usize,
    total_pages: usize,
    /// 見つかった順。ページ順には並べ替えない (走査が先頭から進むので自然に昇順になる)
    pub hits: Vec<Hit>,
}

impl Search {
    pub fn new(query: String, total_pages: usize) -> Self {
        Self {
            query,
            next_page: 0,
            total_pages,
            hits: Vec::new(),
        }
    }

    pub fn is_done(&self) -> bool {
        self.next_page >= self.total_pages
    }

    /// 次に走査するページ範囲を返し、進行位置を進める
    pub fn take_chunk(&mut self, size: usize) -> std::ops::Range<usize> {
        // `next_page` は毎回 `end` 側の clamp を通してしか更新されないため、
        // ここで total_pages を超えて渡ってくる経路は無い。防御的に残しているだけ。
        let start = self.next_page.min(self.total_pages);
        let end = start.saturating_add(size).min(self.total_pages);
        self.next_page = end;
        start..end
    }

    /// 空の一致は積まない (「このページには無かった」を持つ意味がないため)
    pub fn push_hits(&mut self, page: usize, rects: Vec<[f64; 4]>) {
        if rects.is_empty() {
            return;
        }
        self.hits.push(Hit { page, rects });
    }

    pub fn hits_for(&self, page: usize) -> Vec<&Hit> {
        self.hits.iter().filter(|h| h.page == page).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_pages_default_is_6() {
        // 実測 (CHUNK_PAGES のドキュメント参照): groff 製 366 ページの詰まった PDF で
        // 1 ページ 0.7〜0.8ms、32 ページ単位だと最大 35ms/チャンクとなり 8ms の目安を
        // 大きく超えた (32 ページ設定での end-to-end 計測はこの「find_text が支配的」
        // という点までを裏付けたもので、6 という値自体を計測し直したものではない)。
        // 6 は 1 ページあたりの実測レートからの見積もり (0.7〜0.8ms × 6 ≒ 4.2〜4.8ms)
        assert_eq!(CHUNK_PAGES, 6);
    }

    #[test]
    fn chunks_walk_the_document_and_then_stop() {
        let mut s = Search::new("x".into(), 70);
        assert_eq!(s.query, "x");
        let mut start = 0;
        while start < 70 {
            let end = (start + CHUNK_PAGES).min(70);
            assert_eq!(s.take_chunk(CHUNK_PAGES), start..end);
            start = end;
        }
        assert_eq!(start, 70, "端は総ページ数で止まる");
        assert!(s.is_done());
        assert_eq!(s.take_chunk(CHUNK_PAGES), 70..70, "終わったあとは空の範囲");
    }

    #[test]
    fn a_document_with_no_pages_is_done_immediately() {
        let s = Search::new("x".into(), 0);
        assert!(s.is_done());
    }

    #[test]
    fn hits_are_kept_per_page_in_the_order_they_are_found() {
        let mut s = Search::new("x".into(), 3);
        s.push_hits(2, vec![[0.0, 0.0, 0.1, 0.1]]);
        s.push_hits(0, vec![[0.0, 0.2, 0.1, 0.3], [0.0, 0.4, 0.1, 0.5]]);
        assert_eq!(s.hits.len(), 2);
        assert_eq!(s.hits[0].page, 2, "見つかった順に積む");
        assert_eq!(s.hits[1].rects.len(), 2);
    }

    #[test]
    fn empty_hit_lists_are_not_recorded() {
        let mut s = Search::new("x".into(), 3);
        s.push_hits(1, vec![]);
        assert!(s.hits.is_empty());
    }

    #[test]
    fn hits_for_a_page_picks_only_that_page() {
        let mut s = Search::new("x".into(), 3);
        s.push_hits(0, vec![[0.0, 0.0, 0.1, 0.1]]);
        s.push_hits(2, vec![[0.0, 0.2, 0.1, 0.3]]);
        s.push_hits(0, vec![[0.0, 0.6, 0.1, 0.7]]);
        assert_eq!(s.hits_for(0).len(), 2);
        assert_eq!(s.hits_for(1).len(), 0);
        assert_eq!(s.hits_for(2).len(), 1);
    }

    #[test]
    fn chunk_size_zero_does_not_hang() {
        let mut s = Search::new("x".into(), 5);
        assert_eq!(s.take_chunk(0), 0..0);
        assert!(!s.is_done(), "進まないが終わってもいない");
    }
}
