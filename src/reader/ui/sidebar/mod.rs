pub mod annotations;
pub mod outline;
pub mod search;

use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

use crate::reader::outline::OutlineItem;
use crate::reader::ui::ReaderState;

/// 左サイドバー。目次 / 注釈 / 検索 の 3 タブ。
/// タブごとに中身を別ファイルに分けてある (1 ファイルに詰めると借用と参照循環が絡むため)
pub struct Sidebar {
    root: gtk::Box,
    /// タブの切り替えに使う (`Ctrl+F` で検索タブへ移る)
    stack: gtk::Stack,
    annotations: Rc<annotations::Annotations>,
    search: Rc<search::SearchTab>,
    // stack.add_titled は widget を親子付けして強い GObject 参照を取るため、
    // ここで Outline を手放しても widget (行のクロージャ含む) 自体は消えない。
    // それでも持たせているのは stack と同じ理由 (今は使わないが、目次タブを
    // あとから操作する制御に要るため器の時点で持たせておく) で、破棄防止が
    // 目的ではない
    #[allow(dead_code)]
    outline: Option<outline::Outline>,
}

impl Sidebar {
    /// `outline` は目次タブの中身。空なら目次タブ自体を出さない。
    /// **`page` は呼び出し側で総ページ数によりクランプ済みであること**
    /// (`sidebar::outline::Outline::new` のドキュメント参照)
    pub fn new(
        state: Rc<RefCell<ReaderState>>,
        on_jump: Rc<dyn Fn(usize)>,
        on_changed: Rc<dyn Fn()>,
        outline_items: Vec<OutlineItem>,
    ) -> Rc<Self> {
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);

        // annotations が on_jump を消費する前に、目次タブ用に複製しておく
        let outline = if outline_items.is_empty() {
            None
        } else {
            let tab = outline::Outline::new(&outline_items, on_jump.clone());
            stack.add_titled(tab.widget(), Some("outline"), "目次");
            Some(tab)
        };

        // 検索タブも on_jump を使うので、annotations が消費する前に複製しておく
        let search = search::SearchTab::new(on_jump.clone());

        let annotations = annotations::Annotations::new(state, on_jump, on_changed);
        stack.add_titled(annotations.widget(), Some("annotations"), "注釈");

        stack.add_titled(search.widget(), Some("search"), "検索");

        // 起動時に選ばれているのは「注釈」
        stack.set_visible_child_name("annotations");

        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // ここで指定しているのは最小幅の下駄 (280) であって上限ではない。
        // 実際にペインへ反映される最小幅は中身 (StackSwitcher のボタン数) から
        // 決まり、3 タブなら 400px 前後まで自然に広がる。以前はここを 420 に
        // 広げて目次タブのボタンを救おうとしたが、クリップされていたのは
        // 末尾ではなく先頭の「目次」ボタン自身で、原因もこの幅ではなく
        // `ui/mod.rs` 側の `paned.set_position` と `shrink-start-child` の
        // 設定にあった。詳細は `ui/mod.rs` の
        // `paned.set_shrink_start_child(false)` 周辺のコメント参照
        root.set_size_request(280, -1);
        root.append(&switcher);
        root.append(&stack);

        Rc::new(Self { root, stack, annotations, search, outline })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// 検索タブ。走査の実行部を預けたり、`Ctrl+F` / Enter から操作したりする
    pub fn search(&self) -> &Rc<search::SearchTab> {
        &self.search
    }

    /// 検索タブを前面に出す (`Ctrl+F`)
    pub fn show_search(&self) {
        self.stack.set_visible_child_name("search");
    }

    /// 注釈タブを作り直す。注釈が増減したときに呼ぶ
    pub fn refresh(self: &Rc<Self>) {
        self.annotations.refresh();
    }

    /// 指定 ID のメモ欄にフォーカスする (注釈タブ)
    pub fn focus_memo(&self, id: &str) {
        self.annotations.focus_memo(id);
    }
}
