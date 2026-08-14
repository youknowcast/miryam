//! PDF の目次 (アウトライン) を読む。
//!
//! `poppler-rs 0.26` は `IndexIter` の `get_action()` をバインドしていないため、
//! ここだけ FFI を直接叩く (`poppler-rs` が `poppler::ffi` として再輸出している
//! `poppler-sys-rs` を使うので、直接依存は増やさない)。
//! unsafe はこのファイルに閉じ込め、外へは安全な型 (`OutlineItem`) だけを出す。

use gtk4::glib::translate::{Stash, ToGlibPtr};
use poppler::ffi;
use std::ffi::CStr;
use std::os::raw::c_int;

/// 目次の入れ子をたどる深さの上限。
/// 壊れた PDF や循環した目次で無限再帰しないための保険で、
/// これを超えた階層の子は切り捨てる
const MAX_DEPTH: usize = 16;

/// 目次の 1 項目
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    /// 飛び先のページ (0 始まり)。解決できなければ None。
    ///
    /// **総ページ数での検査はしていない**。壊れた (あるいは悪意ある) `/Dest` は
    /// 2 ページの文書でも `Some(9998)` を返せるので、利用側でクランプすること
    pub page: Option<usize>,
    pub children: Vec<OutlineItem>,
}

/// 目次を読む。持たない PDF では空
pub fn read(doc: &poppler::Document) -> Vec<OutlineItem> {
    read_limited(doc, MAX_DEPTH)
}

/// 深さの上限を指定して目次を読む。上限が効いていることをテストから確かめるための入口で、
/// 本番の入口は上限を固定した [`read`]
fn read_limited(doc: &poppler::Document, max_depth: usize) -> Vec<OutlineItem> {
    // Stash は生ポインタを使い終えるまで束縛したままにする (途中で落とさない)
    let stash: Stash<'_, *mut ffi::PopplerDocument, poppler::Document> = doc.to_glib_none();
    let raw_doc = stash.0;
    if raw_doc.is_null() || max_depth == 0 {
        return Vec::new();
    }
    // SAFETY: raw_doc は生存中の `doc` が所有する PopplerDocument。
    // poppler_index_iter_new が返す iter の解放責任は呼び出し側にあるので、
    // walk のあと必ず poppler_index_iter_free する (この間に return はしない)
    unsafe {
        let iter = ffi::poppler_index_iter_new(raw_doc);
        if iter.is_null() {
            // 目次を持たない PDF
            return Vec::new();
        }
        let items = walk(raw_doc, iter, 0, max_depth);
        ffi::poppler_index_iter_free(iter);
        items
    }
}

/// `iter` の階層を末尾まで歩く。`iter` の所有権は呼び出し側に残る
///
/// # Safety
/// `doc` と `iter` は有効な生存中のポインタであること
unsafe fn walk(
    doc: *mut ffi::PopplerDocument,
    iter: *mut ffi::PopplerIndexIter,
    depth: usize,
    max_depth: usize,
) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    loop {
        out.push(unsafe { item_at(doc, iter, depth, max_depth) });
        if unsafe { ffi::poppler_index_iter_next(iter) } == gtk4::glib::ffi::GFALSE {
            break;
        }
    }
    out
}

/// いま指している項目を読む。
/// アクションが取れない・読めない種類でも、タイトルを空にして子は残す
///
/// # Safety
/// `doc` と `iter` は有効な生存中のポインタであること
unsafe fn item_at(
    doc: *mut ffi::PopplerDocument,
    iter: *mut ffi::PopplerIndexIter,
    depth: usize,
    max_depth: usize,
) -> OutlineItem {
    // SAFETY: get_action が返す PopplerAction の解放責任は呼び出し側。
    // action を確保してから poppler_action_free に至るまでの間に return はしない
    let action = unsafe { ffi::poppler_index_iter_get_action(iter) };
    let (title, page) = if action.is_null() {
        (String::new(), None)
    } else {
        // どの種類のアクションでも先頭 2 フィールド (type_, title) の並びは共通
        let any = unsafe { &(*action).any };
        let title = if any.title.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(any.title) }.to_string_lossy().into_owned()
        };
        let page = if any.type_ == ffi::POPPLER_ACTION_GOTO_DEST {
            // dest は action の一部なので個別には解放しない
            let goto = unsafe { &(*action).goto_dest };
            unsafe { page_of(doc, goto.dest) }
        } else {
            None
        };
        unsafe { ffi::poppler_action_free(action) };
        (title, page)
    };

    // 子は別の iter として取り、再帰的に歩く。深さの上限を超えたら取りに行かない
    let children = if depth + 1 >= max_depth {
        Vec::new()
    } else {
        // SAFETY: get_child が返す iter の解放責任も呼び出し側。
        // walk のあと必ず free する (この間に return はしない)
        let child_iter = unsafe { ffi::poppler_index_iter_get_child(iter) };
        if child_iter.is_null() {
            Vec::new()
        } else {
            let c = unsafe { walk(doc, child_iter, depth + 1, max_depth) };
            unsafe { ffi::poppler_index_iter_free(child_iter) };
            c
        }
    };

    OutlineItem { title, page, children }
}

/// `dest` を 0 始まりのページ番号に直す。名前付き飛び先は解決してから読む
///
/// # Safety
/// `doc` は有効な生存中のポインタであること。`dest` は NULL か有効なポインタ
unsafe fn page_of(doc: *mut ffi::PopplerDocument, dest: *mut ffi::PopplerDest) -> Option<usize> {
    if dest.is_null() {
        return None;
    }
    let d = unsafe { &*dest };
    if d.type_ == ffi::POPPLER_DEST_NAMED {
        if d.named_dest.is_null() {
            return None;
        }
        // SAFETY: find_dest が返す PopplerDest の解放責任は呼び出し側。
        // page_num を読んだら即座に free する
        let resolved = unsafe { ffi::poppler_document_find_dest(doc, d.named_dest) };
        if resolved.is_null() {
            return None;
        }
        let n = unsafe { (*resolved).page_num };
        unsafe { ffi::poppler_dest_free(resolved) };
        return zero_based(n);
    }
    zero_based(d.page_num)
}

/// PDF の 1 始まりのページ番号を 0 始まりに直す。範囲外なら None
fn zero_based(page_num: c_int) -> Option<usize> {
    usize::try_from(page_num.checked_sub(1)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// PDF のオブジェクト列から xref 込みの本体を組み立てる。
    /// オフセットは実際に書いたバイト数から計算するので手打ちの数値に頼らない
    /// (`ui::pages` のテストヘルパと同じ手法)
    fn build_pdf(objects: &[String]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(buf.len());
            buf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref_offset = buf.len();
        buf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        buf.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        buf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
                objects.len() + 1
            )
            .as_bytes(),
        );
        buf
    }

    /// 組み立てたバイト列を一時ファイルに書いて poppler で開く
    fn open(dir: &std::path::Path, name: &str, bytes: &[u8]) -> poppler::Document {
        let path = dir.join(name);
        std::fs::File::create(&path).expect("作成できること").write_all(bytes).expect("書けること");
        poppler::Document::from_file(&format!("file://{}", path.display()), None)
            .expect("PDF を開けること")
    }

    /// 1 ページだけ・目次なしの最小 PDF
    fn minimal_pdf_without_outline() -> Vec<u8> {
        build_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>".to_string(),
        ])
    }

    /// 1 ページの PDF に `levels` 段の入れ子目次をぶら下げる。
    /// 各段は子をひとつだけ持ち、いちばん深い段だけ子を持たない
    fn nested_outline_pdf(levels: usize) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>".to_string(),
            // 目次の根。最初の項目はオブジェクト 5
            "<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>".to_string(),
        ];
        for i in 0..levels {
            let obj = 5 + i; // 自分のオブジェクト番号
            let parent = if i == 0 { 4 } else { obj - 1 };
            let child = if i + 1 < levels {
                format!("/First {} 0 R /Last {} 0 R /Count 1 ", obj + 1, obj + 1)
            } else {
                String::new()
            };
            objects.push(format!(
                "<< /Title (Level {i}) /Parent {parent} 0 R {child}\
                 /Dest [3 0 R /XYZ null null null] >>"
            ));
        }
        build_pdf(&objects)
    }

    /// 2 ページの PDF に、**名前付き飛び先** (`/Dest /d1`) を使う目次を 2 項目ぶら下げる。
    /// カタログの `/Dests` 辞書から名前を引く形で、groff や LaTeX が実際に出す形と同じ。
    /// `page_of` の `POPPLER_DEST_NAMED` 分岐 (`find_dest` → `page_num` → `dest_free`) を通す
    fn named_dest_outline_pdf() -> Vec<u8> {
        build_pdf(&[
            // 1: カタログ
            "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /Dests 7 0 R >>".to_string(),
            // 2: ページツリー (2 ページ)
            "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_string(),
            // 3: 1 ページ目
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>".to_string(),
            // 4: 目次の根
            "<< /Type /Outlines /First 5 0 R /Last 8 0 R /Count 2 >>".to_string(),
            // 5: 1 項目目。飛び先は名前 /d1
            "<< /Title (Named one) /Parent 4 0 R /Next 8 0 R /Dest /d1 >>".to_string(),
            // 6: 2 ページ目
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>".to_string(),
            // 7: 名前付き飛び先の辞書
            "<< /d1 [3 0 R /XYZ null null null] /d2 [6 0 R /XYZ null null null] >>".to_string(),
            // 8: 2 項目目。飛び先は名前 /d2
            "<< /Title (Named two) /Parent 4 0 R /Prev 5 0 R /Dest /d2 >>".to_string(),
        ])
    }

    /// 木の最大の深さ (項目が 1 つでもあれば 1)
    fn depth_of(items: &[OutlineItem]) -> usize {
        items.iter().map(|i| 1 + depth_of(&i.children)).max().unwrap_or(0)
    }

    #[test]
    fn a_pdf_without_an_outline_yields_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = open(dir.path(), "no-outline.pdf", &minimal_pdf_without_outline());
        assert!(read(&doc).is_empty(), "目次を持たない PDF では空になること");
    }

    #[test]
    fn a_shallow_outline_is_read_in_full() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = open(dir.path(), "shallow.pdf", &nested_outline_pdf(3));
        let items = read(&doc);

        assert_eq!(items.len(), 1, "根の直下は 1 項目");
        assert_eq!(items[0].title, "Level 0");
        assert_eq!(items[0].page, Some(0), "1 ページ目 (0 始まり)");
        assert_eq!(depth_of(&items), 3, "3 段すべて読めること");
        assert_eq!(items[0].children[0].children[0].title, "Level 2");
    }

    /// 名前付き飛び先 (`/Dest /d1`) が正しいページ番号に解決されること。
    /// LaTeX 由来をはじめ実際の PDF が一番よく通る経路で、`find_dest` が返した
    /// `PopplerDest` を解放する唯一の分岐でもある
    #[test]
    fn a_named_destination_resolves_to_its_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = open(dir.path(), "named.pdf", &named_dest_outline_pdf());
        let items = read(&doc);

        assert_eq!(items.len(), 2, "目次は 2 項目");
        assert_eq!(items[0].title, "Named one");
        assert_eq!(items[0].page, Some(0), "/d1 は 1 ページ目 (0 始まり)");
        assert_eq!(items[1].title, "Named two");
        assert_eq!(items[1].page, Some(1), "/d2 は 2 ページ目 (0 始まり)");
    }

    /// poppler 自身は深さを制限しないこと。
    /// 次のテスト (打ち切り) が「poppler が止めているだけ」で通るのを防ぐ対照
    #[test]
    fn poppler_itself_does_not_limit_the_depth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = open(dir.path(), "deep.pdf", &nested_outline_pdf(24));

        assert_eq!(depth_of(&read_limited(&doc, 24)), 24, "上限を緩めれば 24 段とも読めること");
    }

    /// 深さの上限が実際に打ち切っていること。
    /// フィクスチャは 24 段で固定し、渡した上限のぶんだけ返ることを見る
    #[test]
    fn the_depth_limit_cuts_the_tree_at_the_given_depth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = open(dir.path(), "deep.pdf", &nested_outline_pdf(24));

        assert_eq!(depth_of(&read_limited(&doc, 1)), 1, "1 を渡したら 1 段");
        assert_eq!(depth_of(&read_limited(&doc, 4)), 4, "4 を渡したら 4 段");
        assert_eq!(depth_of(&read_limited(&doc, 16)), 16, "16 を渡したら 16 段");
        assert_eq!(read_limited(&doc, 0), Vec::new(), "0 を渡したら空");
    }

    /// 本番の入口が固定している上限。
    /// 定数を変えたらこのテストが落ちるようにしておく (フィクスチャは 24 段で固定)
    #[test]
    fn read_uses_a_depth_limit_of_sixteen() {
        assert_eq!(MAX_DEPTH, 16, "上限は 16 段");

        let dir = tempfile::tempdir().expect("tempdir");
        let doc = open(dir.path(), "deep.pdf", &nested_outline_pdf(24));

        assert_eq!(depth_of(&read(&doc)), 16, "read は 16 段で打ち切ること");
    }

    #[test]
    fn a_page_number_of_zero_or_less_is_rejected() {
        assert_eq!(zero_based(1), Some(0));
        assert_eq!(zero_based(2), Some(1));
        assert_eq!(zero_based(0), None, "1 始まりなので 0 は不正");
        assert_eq!(zero_based(-1), None);
        assert_eq!(zero_based(c_int::MIN), None, "引き算で溢れないこと");
    }
}
