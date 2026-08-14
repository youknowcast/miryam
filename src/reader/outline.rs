//! PDF の目次 (アウトライン) を読む。
//!
//! `poppler-rs 0.26` は `IndexIter` の `get_action()` をバインドしていないため、
//! ここだけ `poppler-sys-rs` を直接叩く。unsafe はこのファイルに閉じ込め、
//! 外へは安全な型 (`OutlineItem`) だけを出す。

use gtk4::glib::translate::{Stash, ToGlibPtr};
use poppler_sys as ffi;
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
    /// 飛び先のページ (0 始まり)。解決できなければ None
    pub page: Option<usize>,
    pub children: Vec<OutlineItem>,
}

/// 目次を読む。持たない PDF では空
pub fn read(doc: &poppler::Document) -> Vec<OutlineItem> {
    // Stash は生ポインタを使い終えるまで束縛したままにする (途中で落とさない)
    let stash: Stash<'_, *mut ffi::PopplerDocument, poppler::Document> = doc.to_glib_none();
    let raw_doc = stash.0;
    if raw_doc.is_null() {
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
        let items = walk(raw_doc, iter, 0);
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
) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    loop {
        if let Some(item) = unsafe { item_at(doc, iter, depth) } {
            out.push(item);
        }
        if unsafe { ffi::poppler_index_iter_next(iter) } == gtk4::glib::ffi::GFALSE {
            break;
        }
    }
    out
}

/// いま指している項目を読む。読めない種類のアクションなら None
///
/// # Safety
/// `doc` と `iter` は有効な生存中のポインタであること
unsafe fn item_at(
    doc: *mut ffi::PopplerDocument,
    iter: *mut ffi::PopplerIndexIter,
    depth: usize,
) -> Option<OutlineItem> {
    // SAFETY: get_action が返す PopplerAction の解放責任は呼び出し側。
    // 以降 action を使い終えるまで return しない
    let action = unsafe { ffi::poppler_index_iter_get_action(iter) };
    if action.is_null() {
        return None;
    }
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

    // 子は別の iter として取り、再帰的に歩く。深さの上限を超えたら取りに行かない
    let children = if depth + 1 >= MAX_DEPTH {
        Vec::new()
    } else {
        // SAFETY: get_child が返す iter の解放責任も呼び出し側。
        // walk のあと必ず free する (この間に return はしない)
        let child_iter = unsafe { ffi::poppler_index_iter_get_child(iter) };
        if child_iter.is_null() {
            Vec::new()
        } else {
            let c = unsafe { walk(doc, child_iter, depth + 1) };
            unsafe { ffi::poppler_index_iter_free(child_iter) };
            c
        }
    };

    Some(OutlineItem { title, page, children })
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

    #[test]
    fn a_deeper_outline_is_cut_off_at_the_depth_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 上限より深く入れ子にして、切り捨てが効くことを見る
        let doc = open(dir.path(), "deep.pdf", &nested_outline_pdf(MAX_DEPTH + 8));
        let items = read(&doc);

        assert_eq!(depth_of(&items), MAX_DEPTH, "深さは上限で打ち切られること");
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
