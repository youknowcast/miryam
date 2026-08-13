use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

use crate::reader::ui::ReaderState;

/// 左に置く注釈一覧。ページ順に並べ、行ごとに引用文とメモを見せる
pub struct Sidebar {
    root: gtk::Box,
    list: gtk::ListBox,
    state: Rc<RefCell<ReaderState>>,
    on_jump: Rc<dyn Fn(usize)>,
    /// ハイライト ID → メモ入力欄。`focus_memo` が引く
    memo_entries: RefCell<Vec<(String, gtk::TextView)>>,
}

impl Sidebar {
    pub fn new(state: Rc<RefCell<ReaderState>>, on_jump: Rc<dyn Fn(usize)>) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&list));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(280, -1);
        let header = gtk::Label::new(Some("注釈"));
        header.set_margin_top(8);
        header.set_margin_bottom(4);
        root.append(&header);
        root.append(&scrolled);

        let me = Rc::new(Self {
            root,
            list,
            state,
            on_jump,
            memo_entries: RefCell::new(Vec::new()),
        });
        me.refresh();
        me
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// 一覧を作り直す。注釈が増減したときに呼ぶ
    pub fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.memo_entries.borrow_mut().clear();

        // 読み取り専用のときは「書けたように見えて保存されない」状態を作らない
        let read_only = self.state.borrow().read_only;
        let mut items: Vec<(String, usize, String, String)> = self
            .state
            .borrow()
            .sidecar
            .highlights
            .iter()
            .map(|h| (h.id.clone(), h.page, h.quote.clone(), h.memo.clone()))
            .collect();
        // ページ順。同じページ内は作った順のまま (sort_by_key は安定)
        items.sort_by_key(|(_, page, _, _)| *page);

        if items.is_empty() {
            let empty = gtk::Label::new(Some("まだマーカーがありません"));
            empty.set_margin_top(12);
            empty.set_wrap(true);
            self.list.append(&empty);
            return;
        }

        for (id, page, quote, memo) in items {
            self.list.append(&self.build_row(&id, page, &quote, &memo, read_only));
        }
    }

    /// 指定 ID のメモ欄にフォーカスする。マーカーを引いた直後にそのまま書けるように
    pub fn focus_memo(&self, id: &str) {
        let view = self
            .memo_entries
            .borrow()
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, v)| v.clone());
        if let Some(view) = view {
            view.grab_focus();
        }
    }

    fn build_row(
        self: &Rc<Self>,
        id: &str,
        page: usize,
        quote: &str,
        memo: &str,
        read_only: bool,
    ) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        row.set_margin_top(8);
        row.set_margin_bottom(8);
        row.set_margin_start(8);
        row.set_margin_end(8);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let jump = gtk::Button::with_label(&format!("p.{}", page + 1));
        jump.set_halign(gtk::Align::Start);
        jump.set_tooltip_text(Some("このページへ移動します"));
        {
            let on_jump = self.on_jump.clone();
            jump.connect_clicked(move |_| on_jump(page));
        }
        head.append(&jump);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        head.append(&spacer);

        let delete = gtk::Button::with_label("削除");
        delete.set_halign(gtk::Align::End);
        if read_only {
            delete.set_sensitive(false);
            delete.set_tooltip_text(Some("読み取り専用のため削除できません"));
        } else {
            let state = self.state.clone();
            // 強参照だと list → row → button → クロージャ → Sidebar → list の循環になる
            let me = Rc::downgrade(self);
            let id = id.to_string();
            delete.connect_clicked(move |_| {
                state.borrow_mut().remove_highlight(&id);
                // 削除の保存が失敗したことを黙って捨てない (state を借りていない状態で呼ぶ)
                ReaderState::show_save_error(&state);
                if let Some(me) = me.upgrade() {
                    me.refresh();
                }
            });
        }
        head.append(&delete);
        row.append(&head);

        let quote_label = gtk::Label::new(Some(quote));
        quote_label.set_wrap(true);
        quote_label.set_lines(3);
        quote_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        quote_label.set_xalign(0.0);
        quote_label.set_max_width_chars(24);
        quote_label.add_css_class("reader-quote");
        row.append(&quote_label);

        let memo_view = gtk::TextView::new();
        memo_view.set_wrap_mode(gtk::WrapMode::WordChar);
        memo_view.set_size_request(-1, 60);
        memo_view.buffer().set_text(memo);
        memo_view.add_css_class("reader-memo");
        if read_only {
            memo_view.set_editable(false);
            memo_view.set_tooltip_text(Some("読み取り専用のためメモを書けません"));
        } else {
            // set_text のあとで繋ぐ。作り直しのたびに保存し直さないため
            let state = self.state.clone();
            let id = id.to_string();
            memo_view.buffer().connect_changed(move |buf| {
                let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
                state.borrow_mut().set_memo(&id, text);
                ReaderState::show_save_error(&state);
            });
        }
        // 枠を付けて空のメモ欄が見えるようにする
        let memo_frame = gtk::Frame::new(None);
        memo_frame.set_child(Some(&memo_view));
        row.append(&memo_frame);

        self.memo_entries.borrow_mut().push((id.to_string(), memo_view));
        row
    }
}
