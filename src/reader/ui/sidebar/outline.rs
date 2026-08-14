use gtk::prelude::*;
use gtk4 as gtk;
use std::rc::Rc;

use crate::reader::outline::OutlineItem;

/// 左サイドバーの目次タブ。
///
/// 入れ子は `gtk::TreeExpander` を使わず、階層の深さ × 16px の左マージンを
/// 付けた行を並べる**インデント付きの平坦なリスト**で表す (`v4_6` の範囲で
/// 完結し、実装も読みやすいため)。
///
/// **呼び出し側の責務:** `items` の `page` は総ページ数でクランプ済みであること。
/// `outline::read` は `/Dest` をそのまま数値化するだけで総ページ数を見ないため、
/// 壊れた・悪意ある PDF では範囲外のページ番号を返し得る (`outline.rs` のドキュメント
/// コメント参照)。ここではクランプ済みの前提で「`Some` なら飛べる」とだけ扱う
pub struct Outline {
    root: gtk::Box,
}

impl Outline {
    pub fn new(items: &[OutlineItem], on_jump: Rc<dyn Fn(usize)>) -> Self {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        // クリック 1 回でジャンプする (注釈タブの「p.N」ボタンと同じ感覚に揃える)
        list.set_activate_on_single_click(true);

        let mut flat = Vec::new();
        flatten(items, 0, &mut flat);
        // 行の並び順どおりにページ番号だけ抜き出す。クロージャに渡すのに
        // OutlineItem の借用ではなく所有データ (Option<usize>) にしておく
        let pages: Vec<Option<usize>> = flat.iter().map(|(item, _)| item.page).collect();

        for (item, depth) in &flat {
            list.append(&build_row(item, *depth));
        }

        list.connect_row_activated(move |_, row| {
            let Ok(index) = usize::try_from(row.index()) else {
                return;
            };
            if let Some(Some(page)) = pages.get(index) {
                on_jump(*page);
            }
        });

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&list));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(280, -1);
        root.append(&scrolled);

        Self { root }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }
}

/// 木を深さ優先で平らにする。`depth` は行に付けるインデントの段数
///
/// 深さの上限を設けずに再帰する。安全なのは唯一の生成元である `outline::read`
/// (`outline.rs`) が深さ 16 で打ち切っているからで、この関数単体には歯止めが無い
fn flatten<'a>(items: &'a [OutlineItem], depth: usize, out: &mut Vec<(&'a OutlineItem, usize)>) {
    for item in items {
        out.push((item, depth));
        flatten(&item.children, depth + 1, out);
    }
}

fn build_row(item: &OutlineItem, depth: usize) -> gtk::ListBoxRow {
    let title = if item.title.trim().is_empty() { "(無題)".to_string() } else { item.title.clone() };

    let label = gtk::Label::new(Some(&title));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(24);
    label.set_margin_start(8 + (depth as i32) * 16);
    label.set_margin_end(8);
    label.set_margin_top(4);
    label.set_margin_bottom(4);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&label));
    // page が None (クランプ後を含む) の行は飛び先が無いので押せなくする。
    // 非活性の行は row-activated も発火しない
    row.set_sensitive(item.page.is_some());

    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, page: Option<usize>, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem { title: title.to_string(), page, children }
    }

    #[test]
    fn flatten_walks_depth_first_and_tracks_depth() {
        let items = vec![item(
            "A",
            Some(0),
            vec![item("A1", Some(1), vec![]), item("A2", None, vec![])],
        ), item("B", Some(2), vec![])];

        let mut out = Vec::new();
        flatten(&items, 0, &mut out);

        let titles_and_depths: Vec<(&str, usize)> =
            out.iter().map(|(i, d)| (i.title.as_str(), *d)).collect();
        assert_eq!(
            titles_and_depths,
            vec![("A", 0), ("A1", 1), ("A2", 1), ("B", 0)],
            "深さ優先で、子は親の直後に並ぶこと"
        );
    }
}
