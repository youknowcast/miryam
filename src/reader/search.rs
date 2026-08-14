//! 全文検索の進行状態。GTK には触らない。
//!
//! 検索はメインループを止めないよう idle で少しずつ進める。ここはその
//! 「どこまで進んだか」「何が見つかったか」だけを持つ。

/// 1 回の idle で走査するページ数の初期値。
/// **実測して調整すること** (1 回の idle が 8ms を超えないのが目安)
pub const CHUNK_PAGES: usize = 32;

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
    fn chunks_walk_the_document_and_then_stop() {
        let mut s = Search::new("x".into(), 70);
        assert_eq!(s.take_chunk(32), 0..32);
        assert_eq!(s.take_chunk(32), 32..64);
        assert_eq!(s.take_chunk(32), 64..70, "端は総ページ数で止まる");
        assert!(s.is_done());
        assert_eq!(s.take_chunk(32), 70..70, "終わったあとは空の範囲");
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
