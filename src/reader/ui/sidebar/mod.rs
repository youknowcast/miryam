pub mod annotations;

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
    // 今はタブを足すだけで使わないが、Task 5/6 で目次項目クリック時に検索タブへ
    // 切り替えるといった制御に要る。器の時点で持たせておく
    #[allow(dead_code)]
    stack: gtk::Stack,
    annotations: Rc<annotations::Annotations>,
}

impl Sidebar {
    /// `outline` は目次タブの中身 (Task 5 で使う)。今は空かどうかだけ見て、
    /// 目次を持たない PDF ではタブ自体を出さない
    pub fn new(
        state: Rc<RefCell<ReaderState>>,
        on_jump: Rc<dyn Fn(usize)>,
        on_changed: Rc<dyn Fn()>,
        outline: Vec<OutlineItem>,
    ) -> Rc<Self> {
        let annotations = annotations::Annotations::new(state, on_jump, on_changed);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);

        // 目次タブの中身は Task 5 で作る。ここでは持つかどうかだけ見てタブの有無を決める
        if !outline.is_empty() {
            let placeholder = gtk::Label::new(Some("(準備中)"));
            stack.add_titled(&placeholder, Some("outline"), "目次");
        }

        stack.add_titled(annotations.widget(), Some("annotations"), "注釈");

        // 検索タブの中身は Task 6 で作る
        let search_placeholder = gtk::Label::new(Some("(準備中)"));
        stack.add_titled(&search_placeholder, Some("search"), "検索");

        // 起動時に選ばれているのは「注釈」
        stack.set_visible_child_name("annotations");

        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(280, -1);
        root.append(&switcher);
        root.append(&stack);

        Rc::new(Self { root, stack, annotations })
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
