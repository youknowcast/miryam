pub mod annotations;
pub mod outline;

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
    // 今はタブを足すだけで使わないが、Task 6 で検索タブへ切り替えるといった
    // 制御に要る。器の時点で持たせておく
    #[allow(dead_code)]
    stack: gtk::Stack,
    annotations: Rc<annotations::Annotations>,
    // stack は widget() で取った &gtk::Widget しか掴んでいない。Outline 自体は
    // ここで生かしておかないと widget() が指す実体ごと消える (行のクロージャも道連れ)
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

        let annotations = annotations::Annotations::new(state, on_jump, on_changed);
        stack.add_titled(annotations.widget(), Some("annotations"), "注釈");

        // 検索タブの中身は Task 6 で作る
        let search_placeholder = gtk::Label::new(Some("(準備中)"));
        stack.add_titled(&search_placeholder, Some("search"), "検索");

        // 起動時に選ばれているのは「注釈」
        stack.set_visible_child_name("annotations");

        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // 3 タブ (目次/注釈/検索) 分の gtk::StackSwitcher ボタンが収まる最小幅。
        // 2 タブ時代の 280px のままだと 3 つ目のボタンが描画から欠け、
        // 目次タブがクリックで到達できなくなる (実機で確認した実測値に余裕を足した値)
        root.set_size_request(420, -1);
        root.append(&switcher);
        root.append(&stack);

        Rc::new(Self { root, stack, annotations, outline })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
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
