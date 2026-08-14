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

/// `filter` が `None` なら常に一致 (絞り込みなし)。`Some(tag)` なら
/// `tags` にその文字列と全く同じ要素があるときだけ一致する。
/// 空文字列は「タグが無い」ことを表さない — 空 `tags` は空文字列フィルタにも一致しない
fn matches_filter(tags: &[String], filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(want) => tags.iter().any(|t| t == want),
    }
}

/// 渡された `Highlight` 全体からタグを重複なく、初出順に集める (絞り込み欄の選択肢用)
fn collect_tags(highlights: &[crate::reader::store::Highlight]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for h in highlights {
        for tag in &h.tags {
            if seen.insert(tag.clone()) {
                out.push(tag.clone());
            }
        }
    }
    out
}

/// 同じ PDF 内で既に使われているタグを重複なく集める (補完候補用)。
/// `Annotations` のメソッドにせず `state` だけを取るのは、補完のクロージャが
/// `Rc<Annotations>` を強参照で抱え込んで循環を作らないようにするため
fn known_tags(state: &Rc<RefCell<ReaderState>>) -> Vec<String> {
    collect_tags(&state.borrow().sidecar.highlights)
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
    /// タグでの絞り込み UI。「すべて」+ `collect_tags` の結果を並べる
    filter_dropdown: gtk::DropDown,
    /// 絞り込みの選択状態そのもの (`None` は「すべて」)。ドロップダウンの
    /// 選択インデックスではなくタグ文字列で持つのは、`refresh()` のたびに
    /// 選択肢の並びが変わりうるため
    filter: RefCell<Option<String>>,
    /// 直近で `filter_dropdown` に並べたタグ (先頭の「すべて」を除く)。
    /// 選択インデックス → タグ文字列の変換に使う
    filter_options: RefCell<Vec<String>>,
    /// `refresh()` が `filter_dropdown` のモデルや選択を書き換えている間、
    /// その `notify::selected` を「ユーザーが選び直した」と誤認しないためのガード
    updating_filter: std::cell::Cell<bool>,
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

        // 選択肢は refresh() が毎回作り直すので、ここでは空の状態で置いておく
        let filter_dropdown = gtk::DropDown::from_strings(&["すべて"]);
        filter_dropdown.set_margin_start(8);
        filter_dropdown.set_margin_end(8);
        filter_dropdown.set_margin_bottom(4);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(280, -1);
        let header = gtk::Label::new(Some("注釈"));
        header.set_margin_top(8);
        header.set_margin_bottom(4);
        root.append(&header);
        root.append(&filter_dropdown);
        root.append(&scrolled);

        let me = Rc::new(Self {
            root,
            list,
            state,
            on_jump,
            on_changed,
            memo_entries: RefCell::new(Vec::new()),
            filter_dropdown,
            filter: RefCell::new(None),
            filter_options: RefCell::new(Vec::new()),
            updating_filter: std::cell::Cell::new(false),
        });

        // 強参照だと dropdown → クロージャ → Annotations → root → dropdown の循環になる
        // (delete ボタンの connect_clicked と同じ理由)。DropDown 自体は root の子として
        // Annotations と寿命を共にするので、破棄時に別途始末するものは無い
        {
            let weak = Rc::downgrade(&me);
            me.filter_dropdown.connect_selected_notify(move |dd| {
                let Some(me) = weak.upgrade() else {
                    return;
                };
                // refresh() 自身がモデル/選択を書き換えている最中の発火は無視する。
                // ここで refresh() を呼び返すと再入して借用が重なりかねない
                if me.updating_filter.get() {
                    return;
                }
                let selected = dd.selected() as usize;
                let tag = if selected == 0 {
                    None
                } else {
                    me.filter_options.borrow().get(selected - 1).cloned()
                };
                *me.filter.borrow_mut() = tag;
                me.refresh();
            });
        }

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
        let all_tags = collect_tags(&self.state.borrow().sidecar.highlights);

        // 選んでいたタグがもう誰も持っていない (最後の 1 件のタグ欄を書き換えた、など)
        // なら「すべて」に戻す。選んだままにすると選択肢に無い値を保持し続けてしまう
        {
            let mut filter = self.filter.borrow_mut();
            if let Some(tag) = filter.as_ref()
                && !all_tags.iter().any(|t| t == tag)
            {
                *filter = None;
            }
        }
        let filter_value = self.filter.borrow().clone();

        self.sync_filter_dropdown(&all_tags, filter_value.as_deref());

        let mut items: Vec<(String, usize, String, String, Vec<String>)> = self
            .state
            .borrow()
            .sidecar
            .highlights
            .iter()
            .filter(|h| matches_filter(&h.tags, filter_value.as_deref()))
            .map(|h| (h.id.clone(), h.page, h.quote.clone(), h.memo.clone(), h.tags.clone()))
            .collect();
        // ページ順。同じページ内は作った順のまま (sort_by_key は安定)
        items.sort_by_key(|(_, page, _, _, _)| *page);

        if items.is_empty() {
            let text = if filter_value.is_some() {
                "このタグのマーカーはありません"
            } else {
                "まだマーカーがありません"
            };
            let empty = gtk::Label::new(Some(text));
            empty.set_margin_top(12);
            empty.set_wrap(true);
            self.list.append(&empty);
            return;
        }

        for (id, page, quote, memo, tags) in items {
            self.list.append(&self.build_row(&id, page, &quote, &memo, &tags, read_only));
        }
    }

    /// 絞り込み用ドロップダウンの選択肢を最新のタグ一覧に合わせて作り直し、
    /// `filter_value` に対応する項目を選び直す。選択の書き換え自体が
    /// `notify::selected` を発火させるので、その間は `updating_filter` で
    /// ハンドラ側に無視させる
    fn sync_filter_dropdown(&self, all_tags: &[String], filter_value: Option<&str>) {
        self.updating_filter.set(true);

        let mut labels: Vec<&str> = vec!["すべて"];
        labels.extend(all_tags.iter().map(String::as_str));
        let model = gtk::StringList::new(&labels);
        self.filter_dropdown.set_model(Some(&model));

        let selected = match filter_value {
            None => 0,
            Some(tag) => all_tags.iter().position(|t| t == tag).map(|i| i + 1).unwrap_or(0),
        };
        self.filter_dropdown.set_selected(selected as u32);

        *self.filter_options.borrow_mut() = all_tags.to_vec();

        self.updating_filter.set(false);
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

    #[test]
    fn no_filter_matches_everything() {
        assert!(matches_filter(&[], None));
        assert!(matches_filter(&["重要".into()], None));
    }

    #[test]
    fn a_filter_matches_only_rows_carrying_that_tag() {
        let tags = vec!["重要".to_string(), "あとで".to_string()];
        assert!(matches_filter(&tags, Some("重要")));
        assert!(!matches_filter(&tags, Some("保留")));
        assert!(!matches_filter(&[], Some("重要")), "タグ無しは絞り込みに残らない");
    }

    /// タグだけを差し替えた Highlight を作る。他のフィールドはこのテストでは見ない
    fn hl(tags: &[&str]) -> crate::reader::store::Highlight {
        crate::reader::store::Highlight {
            id: "x".into(),
            page: 0,
            color: "yellow".into(),
            rects: vec![[0.0, 0.0, 0.1, 0.1]],
            quote: "q".into(),
            memo: String::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            llm: vec![],
            created_at: chrono::Local::now(),
        }
    }

    #[test]
    fn collect_tags_is_deduped_and_in_first_seen_order() {
        let hs = vec![hl(&["b", "a"]), hl(&["a", "c"]), hl(&[])];
        assert_eq!(collect_tags(&hs), vec!["b", "a", "c"]);
    }

    /// フィルタが空文字列のとき (タグ欄が空のまま選ばれることは無いはずだが、
    /// 空文字列という値そのものは「一致しない」を返す健全な振る舞いにしておく)
    #[test]
    fn empty_string_filter_matches_nothing_unless_the_tag_itself_is_empty() {
        assert!(!matches_filter(&["重要".into()], Some("")));
    }

    #[test]
    fn collect_tags_of_no_highlights_is_empty() {
        assert!(collect_tags(&[]).is_empty());
    }

    /// 大文字小文字を区別する (parse_tags と同じ扱い)。ここを崩すと
    /// `t.eq_ignore_ascii_case(want)` のような変異が素通りしてしまう
    #[test]
    fn matches_filter_is_case_sensitive() {
        assert!(!matches_filter(&["A".into()], Some("a")));
        assert!(matches_filter(&["a".into()], Some("a")));
    }

    /// collect_tags も大文字小文字を同一視しない (parse_tags と同じ扱い)
    #[test]
    fn collect_tags_is_case_sensitive() {
        let hs = vec![hl(&["A"]), hl(&["a"])];
        assert_eq!(collect_tags(&hs), vec!["A", "a"]);
    }
}
