use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

use crate::reader::ui::ReaderState;

/// カンマ区切りのタグ入力を正規化する: 前後の空白を落とし、空要素を捨て、
/// 重複を落とす (先に出てきたものを残す)。GTK に依存しない純粋関数
fn parse_tags(input: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in input.split(',') {
        let tag = part.trim();
        if tag.is_empty() {
            continue;
        }
        if seen.insert(tag.to_string()) {
            out.push(tag.to_string());
        }
    }
    out
}

/// 同じ PDF 内で既に使われているタグを重複なく集める (補完候補用)。
/// `Annotations` のメソッドにせず `state` だけを取るのは、補完のクロージャが
/// `Rc<Annotations>` を強参照で抱え込んで循環を作らないようにするため
fn known_tags(state: &Rc<RefCell<ReaderState>>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for h in &state.borrow().sidecar.highlights {
        for tag in &h.tags {
            if seen.insert(tag.clone()) {
                out.push(tag.clone());
            }
        }
    }
    out
}

/// 左に置く注釈一覧。ページ順に並べ、行ごとに引用文とメモを見せる
pub struct Annotations {
    root: gtk::Box,
    list: gtk::ListBox,
    state: Rc<RefCell<ReaderState>>,
    on_jump: Rc<dyn Fn(usize)>,
    /// 注釈を書き換えたのでページを描き直してほしい、と頼む。
    /// どのページの行からでも削除できるので全ページが対象になる
    on_changed: Rc<dyn Fn()>,
    /// ハイライト ID → メモ入力欄。`focus_memo` が引く
    memo_entries: RefCell<Vec<(String, gtk::TextView)>>,
}

impl Annotations {
    pub fn new(
        state: Rc<RefCell<ReaderState>>,
        on_jump: Rc<dyn Fn(usize)>,
        on_changed: Rc<dyn Fn()>,
    ) -> Rc<Self> {
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
            on_changed,
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
        let mut items: Vec<(String, usize, String, String, Vec<String>)> = self
            .state
            .borrow()
            .sidecar
            .highlights
            .iter()
            .map(|h| (h.id.clone(), h.page, h.quote.clone(), h.memo.clone(), h.tags.clone()))
            .collect();
        // ページ順。同じページ内は作った順のまま (sort_by_key は安定)
        items.sort_by_key(|(_, page, _, _, _)| *page);

        if items.is_empty() {
            let empty = gtk::Label::new(Some("まだマーカーがありません"));
            empty.set_margin_top(12);
            empty.set_wrap(true);
            self.list.append(&empty);
            return;
        }

        for (id, page, quote, memo, tags) in items {
            self.list.append(&self.build_row(&id, page, &quote, &memo, &tags, read_only));
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
        tags: &[String],
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
            // 現状は到達しない (読み取り専用になるのは sidecar が読めなかったときだけで、
            // そのとき highlights は必ず空なので行が作られない)。将来のための一貫した扱い
            delete.set_sensitive(false);
            delete.set_tooltip_text(Some("読み取り専用のため削除できません"));
        } else {
            let state = self.state.clone();
            let on_changed = self.on_changed.clone();
            // 強参照だと list → row → button → クロージャ → Annotations → list の循環になる
            let me = Rc::downgrade(self);
            let id = id.to_string();
            delete.connect_clicked(move |_| {
                state.borrow_mut().remove_highlight(&id);
                // 削除の保存が失敗したことを黙って捨てない (state を借りていない状態で呼ぶ)
                ReaderState::show_save_error(&state);
                // 行を消すだけではページ上の塗りが残るので、描き直しも頼む
                on_changed();
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
            // 現状は到達しない (上の削除ボタンと同じ理由)
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

        let tags_entry = gtk::Entry::new();
        tags_entry.set_placeholder_text(Some("タグ (カンマ区切り)"));
        tags_entry.set_text(&tags.join(", "));
        tags_entry.add_css_class("reader-tags");
        if read_only {
            // 現状は到達しない (削除ボタン・メモ欄と同じ理由)
            tags_entry.set_sensitive(false);
            tags_entry.set_tooltip_text(Some("読み取り専用のためタグを書けません"));
        } else {
            // set_text のあとで繋ぐ。作り直しのたびに保存し直さないため (メモ欄と同じ流儀)
            {
                let state = self.state.clone();
                let id = id.to_string();
                tags_entry.connect_changed(move |entry| {
                    let tags = parse_tags(&entry.text());
                    state.borrow_mut().set_tags(&id, tags);
                    ReaderState::show_save_error(&state);
                });
            }
            // 既存タグの候補を Popover で出す (EntryCompletion は GTK4 で非推奨方向のため
            // 使わない)。候補が無ければ何も出さない。フォーカスを得たときだけ開き、
            // 閉じるのは Popover 自身の autohide に任せる (entry 側で焦点喪失時に
            // popdown を打つと、候補ボタンをクリックした瞬間の焦点移動と競合しかねない)
            {
                let state = self.state.clone();
                let popover_slot: Rc<RefCell<Option<gtk::Popover>>> = Rc::new(RefCell::new(None));
                // entry が壊れるときにここから外す (pages.rs の area.connect_destroy と同じ理由:
                // Popover 自身の connect_closed が popover_slot を強参照するので、::closed が
                // 発火するまで自分で自分を生かし続ける。refresh() は行ごと tags_entry を
                // list.remove で捨てるが、それだけでは ::closed は同期的に発火しないため、
                // ここで明示的に slot を空にして popdown + unparent する
                {
                    let slot = popover_slot.clone();
                    tags_entry.connect_destroy(move |_| {
                        let open = slot.borrow_mut().take();
                        if let Some(p) = open {
                            p.popdown();
                            if p.parent().is_some() {
                                p.unparent();
                            }
                        }
                    });
                }
                tags_entry.connect_has_focus_notify(move |entry| {
                    if !entry.has_focus() {
                        return;
                    }
                    // 前に開いたままのものが残っていたら片づける (通常は autohide が
                    // 先に閉じているはずだが、念のため new_popover と同じ流儀で防御する)
                    if let Some(p) = popover_slot.borrow_mut().take() {
                        p.popdown();
                        if p.parent().is_some() {
                            p.unparent();
                        }
                    }

                    let current = parse_tags(&entry.text());
                    let candidates: Vec<String> =
                        known_tags(&state).into_iter().filter(|t| !current.contains(t)).collect();
                    if candidates.is_empty() {
                        return;
                    }

                    let popover = gtk::Popover::new();
                    popover.set_parent(entry);
                    popover.set_autohide(true);
                    // Popover が自分で閉じたら親から外す。放っておくと親に残り続ける
                    {
                        let popover_slot = popover_slot.clone();
                        popover.connect_closed(move |p| {
                            let _ = popover_slot.borrow_mut().take();
                            if p.parent().is_some() {
                                p.unparent();
                            }
                        });
                    }

                    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    for tag in candidates {
                        let button = gtk::Button::with_label(&tag);
                        // 強参照だと popover → box → button → クロージャ → entry/popover の
                        // 循環になりかねないので、両方とも弱参照で持つ
                        let entry_weak = entry.downgrade();
                        let popover_weak = popover.downgrade();
                        button.connect_clicked(move |_| {
                            let Some(entry) = entry_weak.upgrade() else {
                                return;
                            };
                            let mut current = parse_tags(&entry.text());
                            if !current.iter().any(|t| t == &tag) {
                                current.push(tag.clone());
                            }
                            entry.set_text(&current.join(", "));
                            entry.set_position(-1);
                            if let Some(p) = popover_weak.upgrade() {
                                p.popdown();
                            }
                            entry.grab_focus();
                        });
                        list.append(&button);
                    }
                    popover.set_child(Some(&list));

                    popover_slot.replace(Some(popover.clone()));
                    popover.popup();
                });
            }
        }
        row.append(&tags_entry);

        self.memo_entries.borrow_mut().push((id.to_string(), memo_view));
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tags_trims_and_drops_empties() {
        assert_eq!(parse_tags(" 重要 , あとで読む ,, "), vec!["重要", "あとで読む"]);
    }

    #[test]
    fn parse_tags_drops_duplicates_keeping_the_first() {
        assert_eq!(parse_tags("a, b, a"), vec!["a", "b"]);
    }

    #[test]
    fn parse_tags_of_an_empty_string_is_empty() {
        assert!(parse_tags("   ").is_empty());
    }

    /// 大文字小文字は別のタグとして残す (小文字化して同一視しない)
    #[test]
    fn parse_tags_is_case_sensitive() {
        assert_eq!(parse_tags("A, a"), vec!["A", "a"]);
    }

    /// 全角スペース (Unicode の空白) も前後から落ちる。ASCII スペースだけを見る
    /// 実装 (例: trim_matches(' ')) への変異を捕まえる
    #[test]
    fn parse_tags_trims_full_width_space() {
        assert_eq!(parse_tags("　重要　"), vec!["重要"]);
    }
}
