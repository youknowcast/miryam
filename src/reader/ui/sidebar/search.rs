use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::reader::search::Hit;
use crate::reader::ui_logic::{STATUS_IDLE, advance, status_text};

/// 走査の実行部。検索語を受け取って走査を始める
pub type Runner = Rc<dyn Fn(String)>;

/// 左サイドバーの検索タブ。
///
/// **探すのは PDF 本文だけ** (自分のメモや引用は対象外)。
///
/// ここは「見つかったものを並べて、そこへ飛ばす」だけの器で、走査そのものは持たない。
/// 走査は poppler の文書を握っている `ui::mod` 側にあり、`set_runner` でここに預けられる
/// (`start` はそれを呼ぶだけ)。分けているのは、走査が idle で少しずつ進む都合上
/// `Search` の借用と GTK の更新が混ざりやすく、両方を 1 か所に置くと絡むため
pub struct SearchTab {
    root: gtk::Box,
    /// 一致の一覧。行の並びは `hits` の並びとそのまま対応する (`row_at_index` で引く)
    list: gtk::ListBox,
    /// 「検索中…」「N 件見つかりました」を出すところ
    status: gtk::Label,
    on_jump: Rc<dyn Fn(usize)>,
    /// 走査の実行部。`ui::mod` が `set_runner` で預ける。
    /// 実行部は SearchTab を弱参照でしか持たないので循環しない
    runner: RefCell<Option<Runner>>,
    /// 表示中の一致 (見つかった順 = ページ昇順)
    hits: RefCell<Vec<Hit>>,
    /// `hits` に積まれた `rects` の総数の走行合計。`update_status` が `push` の
    /// たびに呼ばれるので、そのつど `hits` 全体を畳み込むと O(件数²) になる。
    /// ここに走行合計を持てば `update_status` は 1 行で済む
    total_marks: Cell<usize>,
    /// いま何番目の一致にいるか。行のクロージャからも書くので `Rc` で共有する
    /// (`Rc<Self>` を配ると list → 行 → クロージャ → SearchTab → list の循環になる)
    current: Rc<Cell<Option<usize>>>,
    running: Cell<bool>,
    /// 一度でも検索したか。「まだ検索していない」と「見つからなかった」を書き分けるため
    searched: Cell<bool>,
}

impl SearchTab {
    pub fn new(on_jump: Rc<dyn Fn(usize)>) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&list));

        let status = gtk::Label::new(Some(STATUS_IDLE));
        status.set_wrap(true);
        status.set_xalign(0.0);
        status.set_margin_top(8);
        status.set_margin_bottom(4);
        status.set_margin_start(8);
        status.set_margin_end(8);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(280, -1);
        root.append(&status);
        root.append(&scrolled);

        Rc::new(Self {
            root,
            list,
            status,
            on_jump,
            runner: RefCell::new(None),
            hits: RefCell::new(Vec::new()),
            total_marks: Cell::new(0),
            current: Rc::new(Cell::new(None)),
            running: Cell::new(false),
            searched: Cell::new(false),
        })
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// 走査の実行部を預ける。窓を組み立てるときに 1 回だけ呼ぶ
    pub fn set_runner(&self, runner: Runner) {
        *self.runner.borrow_mut() = Some(runner);
    }

    /// 走査を始める。**進行中のものがあれば捨てる** (捨てる責任は実行部側の世代番号にある)。
    /// 空の検索語なら一覧を空にするだけで走査はしない (実行部がページ上の強調を消す)
    pub fn start(&self, query: String) {
        self.clear();
        let has_query = !query.trim().is_empty();
        self.searched.set(has_query);
        self.running.set(has_query);
        self.update_status();
        // 借用を持ったまま実行部を呼ばない (実行部はここの `push` を呼び得る)
        let runner = self.runner.borrow().clone();
        if let Some(runner) = runner {
            runner(query);
        }
    }

    /// 一致を 1 件足して、行を 1 つ増やす。**走査の途中で 1 チャンクごとに呼ばれる**
    pub fn push(&self, hit: Hit) {
        // 借用は 1 文で落とす。以降はコピーした値だけで組み立てる
        let index = self.hits.borrow().len();
        let (page, count) = (hit.page, hit.rects.len());
        self.hits.borrow_mut().push(hit);
        self.total_marks.set(self.total_marks.get() + count);
        self.list.append(&self.build_row(index, page, count));
        self.update_status();
    }

    /// 走査中かどうか。終わったら `false` にすると件数の表示に変わる
    pub fn set_running(&self, running: bool) {
        self.running.set(running);
        self.update_status();
    }

    /// 次の一致へ飛ぶ (末尾まで行ったら先頭へ回る)
    pub fn next(&self) {
        self.step(1);
    }

    /// 前の一致へ飛ぶ (先頭まで行ったら末尾へ回る)
    pub fn prev(&self) {
        self.step(-1);
    }

    /// 一覧と一致を捨てる。ページ上の強調を消すのは実行部の仕事
    fn clear(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.hits.borrow_mut().clear();
        self.total_marks.set(0);
        self.current.set(None);
    }

    fn step(&self, delta: isize) {
        // 「次はどれか」の判断は `ui_logic::advance` にある (GTK 非依存・テスト付き)。
        // ここは借用を短く保って結果をウィジェットへ流すだけ
        let len = self.hits.borrow().len();
        let Some(index) = advance(self.current.get(), len, delta) else {
            return;
        };
        // 借用は 1 文で落とす。`on_jump` はページ側を触るので借りたまま呼ばない
        let page = self.hits.borrow().get(index).map(|h| h.page);
        let Some(page) = page else {
            return;
        };
        self.current.set(Some(index));
        select_row(&self.list, index);
        (self.on_jump)(page);
    }

    fn update_status(&self) {
        // どの文言を選ぶかの判断は `ui_logic::status_text` にある (GTK 非依存・テスト付き)
        let pages = self.hits.borrow().len();
        let marks = self.total_marks.get();
        let text = status_text(self.running.get(), self.searched.get(), pages, marks);
        self.status.set_text(&text);
    }

    /// 1 ページ分の一致を表す行。押すとそのページへ飛ぶ (注釈タブの「p.N」ボタンと同じ操作感)
    fn build_row(&self, index: usize, page: usize, count: usize) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        row.set_margin_start(8);
        row.set_margin_end(8);

        let jump = gtk::Button::with_label(&format!("p.{}", page + 1));
        jump.set_tooltip_text(Some("このページへ移動します"));
        {
            let on_jump = self.on_jump.clone();
            let current = self.current.clone();
            // ここで `Rc<SearchTab>` を掴むと list → 行 → ボタン → クロージャ → SearchTab
            // → list の循環になる。必要なのは「何番目か」の記録と飛び先だけなので、
            // 共有する Cell とジャンプ用のクロージャだけを持つ
            jump.connect_clicked(move |_| {
                current.set(Some(index));
                on_jump(page);
            });
        }
        row.append(&jump);

        let label = gtk::Label::new(Some(&format!("{count} 件")));
        label.set_xalign(0.0);
        row.append(&label);

        row
    }
}

/// `index` 番目の行を選択状態にする。行が無ければ何もしない
fn select_row(list: &gtk::ListBox, index: usize) {
    let Ok(index) = i32::try_from(index) else {
        return;
    };
    if let Some(row) = list.row_at_index(index) {
        list.select_row(Some(&row));
    }
}
