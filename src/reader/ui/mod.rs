pub mod pages;
pub mod sidebar;

use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use crate::reader::geom;
use crate::reader::search;
use crate::reader::store::{self, Highlight, Sidecar};

/// ページ間の隙間 (px)
const PAGE_GAP: f64 = 12.0;

/// しおりのページへスクロールを予約する。呼び出し時点でレイアウトが確定している
/// (upper/page_size が実寸を反映している) ことが前提。`changed` ハンドラ経由・
/// レイアウトが最初から確定していた即時経路のどちらからも使う共通処理
fn schedule_bookmark_scroll(view: &pages::PageView, scrolled: &gtk::ScrolledWindow, bookmark: usize) {
    let offsets = geom::page_offsets(&view.scaled_heights(), PAGE_GAP);
    let Some(&y) = offsets.get(bookmark) else {
        return;
    };
    // ScrolledWindow (延いては vadjustment) を次の 1 ターンまで強参照で持ち越さない。
    // 弱参照にして upgrade できなければ何もしない (このファイルの他のクロージャと同じ流儀)
    let scrolled_weak = scrolled.downgrade();
    glib::idle_add_local_once(move || {
        let Some(scrolled) = scrolled_weak.upgrade() else {
            return;
        };
        // 最終ページ近くのしおりでは y + MARGIN が upper - page_size を超えることも
        // あるが、それは GTK が最大までクランプしてくれればよい (それが正しい挙動)
        scrolled.vadjustment().set_value(y + geom::MARGIN);
    });
}

/// 開いている PDF とその注釈。UI 全体で 1 つを共有する
pub struct ReaderState {
    pub pdf_path: PathBuf,
    pub sidecar: Sidecar,
    pub colors: Vec<String>,
    seq: u32,
    /// サイドカーが壊れていたときは読み取り専用にして上書きしない
    pub read_only: bool,
    /// 直近の保存失敗。`show_save_error` が拾って画面に出す
    save_error: Option<String>,
    /// 画面上部の警告バー。保存に失敗したことを黙って握り潰さないために持つ
    warn_bar: Option<gtk::Label>,
}

impl ReaderState {
    fn build(pdf_path: PathBuf, sidecar: Sidecar, colors: Vec<String>, read_only: bool) -> Self {
        Self {
            pdf_path,
            sidecar,
            colors,
            seq: 0,
            read_only,
            save_error: None,
            warn_bar: None,
        }
    }

    /// 開くのに失敗したときは読み取り専用にし、警告文を一緒に返す
    pub fn open(pdf_path: PathBuf, colors: Vec<String>) -> anyhow::Result<(Self, Option<String>)> {
        match Sidecar::load(&pdf_path) {
            Ok(Some(sc)) => Ok((Self::build(pdf_path, sc, colors, false), None)),
            Ok(None) => {
                let sc = Sidecar::new(&pdf_path)?;
                Ok((Self::build(pdf_path, sc, colors, false), None))
            }
            Err(e) => {
                let warn = format!("{e:#}");
                eprintln!("miryam-reader: {warn}");
                let sc = Sidecar::new(&pdf_path)?;
                Ok((Self::build(pdf_path, sc, colors, true), Some(warn)))
            }
        }
    }

    /// 警告バーを預ける。以後 `show_save_error` がここに書き込む
    pub fn set_warn_bar(&mut self, bar: gtk::Label) {
        self.warn_bar = Some(bar);
    }

    /// 直近の保存が失敗したままかどうか
    pub fn save_failed(&self) -> bool {
        self.save_error.is_some()
    }

    /// 直近の保存の結果を警告バーに反映する。成功していれば消す。
    /// **state を借りたまま呼ばないこと** (中で `borrow` する)
    pub fn show_save_error(state: &Rc<RefCell<Self>>) {
        let (msg, read_only, bar) = {
            let st = state.borrow();
            (st.save_error.clone(), st.read_only, st.warn_bar.clone())
        };
        let Some(bar) = bar else {
            return;
        };
        match msg {
            Some(msg) => {
                bar.set_text(&format!("注釈を保存できません: {msg}"));
                bar.set_visible(true);
            }
            // 読み取り専用の警告は保存失敗とは別物なので消さない
            // (読み取り専用では save() が何もしないので save_error も立たない)
            None if !read_only => {
                bar.set_text("");
                bar.set_visible(false);
            }
            None => {}
        }
    }

    pub fn add_highlight(
        &mut self,
        page: usize,
        color: &str,
        rects: Vec<[f64; 4]>,
        quote: String,
    ) -> String {
        let now = chrono::Local::now();
        let id = store::new_id(now, self.seq);
        self.seq = self.seq.wrapping_add(1);
        self.sidecar.highlights.push(Highlight {
            id: id.clone(),
            page,
            color: color.to_string(),
            rects,
            quote,
            memo: String::new(),
            tags: Vec::new(),
            llm: Vec::new(),
            created_at: now,
        });
        self.save();
        id
    }

    pub fn remove_highlight(&mut self, id: &str) {
        self.sidecar.highlights.retain(|h| h.id != id);
        self.save();
    }

    /// メモを書き換える。中身が変わっていなければ書き込まない
    pub fn set_memo(&mut self, id: &str, memo: String) {
        let Some(h) = self.sidecar.highlights.iter_mut().find(|h| h.id == id) else {
            return;
        };
        if h.memo == memo {
            return;
        }
        h.memo = memo;
        self.save();
    }

    /// 失敗しても落とさない。結果は `save_error` に残して `show_save_error` が画面に出す
    /// (成功したら消す。古い失敗の表示を残さないため)
    pub fn save(&mut self) {
        if self.read_only {
            return;
        }
        match self.sidecar.save(&self.pdf_path) {
            Ok(()) => self.save_error = None,
            Err(e) => {
                let msg = format!("{e:#}");
                eprintln!("miryam-reader: 注釈を保存できません: {msg}");
                self.save_error = Some(msg);
            }
        }
    }
}

pub fn run(path: PathBuf) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.youknow.miryam.reader")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let path = Rc::new(path);
    app.connect_activate(move |app| {
        if let Err(e) = build_window(app, &path) {
            eprintln!("miryam-reader: {e:#}");
            show_fatal(app, &format!("{e:#}"));
        }
    });
    app.run_with_args::<&str>(&[])
}

/// 目次の `page` を総ページ数でクランプする。範囲外なら「飛べない」扱いにする
/// (`None` にする) ため、`outline::read` が返し得る範囲外のページ番号が
/// サイドバーより先に UI へ渡らないようにする関所
///
/// ここは深さの上限を設けずに `children` を再帰的にたどる。安全なのは唯一の
/// 生成元である `outline::read` (`outline.rs`) が深さ 16 で打ち切っているから
/// で、この関数単体には歯止めが無い
fn clamp_outline(
    items: Vec<crate::reader::outline::OutlineItem>,
    total_pages: usize,
) -> Vec<crate::reader::outline::OutlineItem> {
    items
        .into_iter()
        .map(|item| {
            let crate::reader::outline::OutlineItem { title, page, children } = item;
            crate::reader::outline::OutlineItem {
                title,
                page: page.filter(|&p| p < total_pages),
                children: clamp_outline(children, total_pages),
            }
        })
        .collect()
}

/// 全文検索の走査部を組み立てて検索タブへ預ける。**探すのは PDF 本文だけ**
///
/// メインループを止めないよう、`glib::idle_add_local` で 1 回につき
/// `search::CHUNK_PAGES` ページずつ進む。一致は出るたびにタブへ足すので、
/// 走査が全部終わるのを待たずに結果が並び始める
///
/// **借用の約束:** `current` (`Search`) を借りたまま `find_text` も UI 更新も呼ばない。
/// `Search::hits_for` のように借用を返す API を歩いている最中に `push_hits` を呼ぶと
/// `BorrowMutError` になり、idle (= FFI の向こう) から呼ばれているので abort する。
/// 借用は「範囲を取り出すブロック」と「結果を積んで写し取るブロック」の 2 つに閉じ、
/// どちらもコピーした値だけを外へ渡す
fn install_search(
    doc: &poppler::Document,
    tab: &Rc<sidebar::search::SearchTab>,
    view: &Rc<pages::PageView>,
) {
    // 走査でページを 1 枚ずつ触るので手元に持っておく (GObject の clone は参照カウント
    // 増加なので、PageView が持っているのと同じページを指す)。読めないページに当たったら
    // そこで打ち切り、その手前までを検索対象にする (`PageView::new` が全ページを読めて
    // いなければ窓自体が開かないので、実際には起こらないはず)
    let mut collected = Vec::new();
    for i in 0..doc.n_pages() {
        let Some(page) = doc.page(i) else {
            eprintln!("miryam-reader: {} ページ目が読めないので検索対象から外します", i + 1);
            break;
        };
        collected.push(page);
    }
    let pages = Rc::new(collected);

    // 検索を始めるたびに 1 つ進める。古い idle は自分の世代と食い違ったら黙って降りる
    let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let current: Rc<RefCell<Option<search::Search>>> = Rc::new(RefCell::new(None));

    // このクロージャは SearchTab が持つので、tab を強参照で掴むと
    // SearchTab → runner → SearchTab の循環になる。view も同じ理由で弱参照にする
    let tab_weak = Rc::downgrade(tab);
    let view_weak = Rc::downgrade(view);
    let runner: sidebar::search::Runner = Rc::new(move |query: String| {
        // 進行中の走査を捨てる。古い idle が生きていても世代が違えば何もせず降りる
        let this_generation = generation.get().wrapping_add(1);
        generation.set(this_generation);
        // 前の検索の強調を消す
        if let Some(view) = view_weak.upgrade() {
            view.set_search_hits(Vec::new());
        }
        if query.trim().is_empty() {
            *current.borrow_mut() = None;
            return;
        }
        *current.borrow_mut() = Some(search::Search::new(query, pages.len()));

        let pages = pages.clone();
        let generation = generation.clone();
        let current = current.clone();
        let tab_weak = tab_weak.clone();
        let view_weak = view_weak.clone();
        glib::idle_add_local(move || {
            // 自分より新しい検索が始まっていたらここで終わり
            if generation.get() != this_generation {
                return glib::ControlFlow::Break;
            }
            // 窓が閉じたあとまで走り続けない
            let (Some(tab), Some(view)) = (tab_weak.upgrade(), view_weak.upgrade()) else {
                return glib::ControlFlow::Break;
            };

            // (1) 走査する範囲と検索語を取り出す。借用はこのブロックの中だけ
            let chunk = {
                let mut slot = current.borrow_mut();
                slot.as_mut().map(|s| (s.query.clone(), s.take_chunk(search::CHUNK_PAGES)))
            };
            let Some((query, range)) = chunk else {
                return glib::ControlFlow::Break;
            };

            // (2) 借用を持たずに poppler を叩く
            let mut found = Vec::new();
            for index in range {
                let Some(page) = pages.get(index) else {
                    continue;
                };
                let rects = pages::find_hits(page, &query);
                if !rects.is_empty() {
                    found.push(search::Hit { page: index, rects });
                }
            }

            // (3) 見つかった分を積み、進行状況を写し取る。借用はこのブロックの中だけ
            let (all_hits, done) = {
                let mut slot = current.borrow_mut();
                let Some(s) = slot.as_mut() else {
                    return glib::ControlFlow::Break;
                };
                for hit in &found {
                    s.push_hits(hit.page, hit.rects.clone());
                }
                // 何も見つからなかったチャンクでは強調の入れ替えごと省く
                let all = (!found.is_empty()).then(|| s.hits.clone());
                (all, s.is_done())
            };

            // (4) ここではもう何も借りていない。UI を触ってよい
            for hit in found {
                tab.push(hit);
            }
            if let Some(all) = all_hits {
                view.set_search_hits(all);
            }
            if done {
                tab.set_running(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    });
    tab.set_runner(runner);
}

/// 検索欄の Enter / Shift+Enter と、窓全体の `Ctrl+F` を配線する
fn wire_search_keys(
    window: &gtk::ApplicationWindow,
    entry: &gtk::SearchEntry,
    sidebar: &Rc<sidebar::Sidebar>,
) {
    // 最後に走査を始めた語。Enter を「新しく検索する」と「次の一致へ飛ぶ」で使い分ける
    let last_query: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    {
        let tab = sidebar.search().clone();
        let sidebar = sidebar.clone();
        let last_query = last_query.clone();
        // entry → ハンドラ → Sidebar (→ SearchTab) の向きしかない。Sidebar 側は
        // ツールバーの entry を知らないので循環しない
        entry.connect_activate(move |entry| {
            let query = entry.text().to_string();
            // 借用は 1 文で落とす
            let same = *last_query.borrow() == query;
            if same {
                tab.next();
                return;
            }
            last_query.replace(query.clone());
            sidebar.show_search();
            tab.start(query);
        });
    }

    {
        // Shift+Enter で前の一致へ。SearchEntry の既定では Enter (activate) しか
        // 拾えないので、修飾キー付きだけをここで横取りする
        let tab = sidebar.search().clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, state| {
            let enter = matches!(
                keyval,
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter | gtk::gdk::Key::ISO_Enter
            );
            if enter && state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                tab.prev();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(key);
    }

    {
        // Ctrl+F: 検索タブを出して検索欄へフォーカスを移す。ページ側のジェスチャや
        // 他のウィジェットに先に食われないよう capture 相で受ける
        let sidebar = sidebar.clone();
        // 窓 → コントローラ → クロージャ → entry と辿れてしまうと、entry は窓の子なので
        // 参照が輪になる。弱参照で持ち、取れなければ何もしない
        let entry_weak = entry.downgrade();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, keyval, _, state| {
            if !state.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                || !matches!(keyval, gtk::gdk::Key::f | gtk::gdk::Key::F)
            {
                return glib::Propagation::Proceed;
            }
            sidebar.show_search();
            if let Some(entry) = entry_weak.upgrade() {
                entry.grab_focus();
            }
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }
}

fn build_window(app: &gtk::Application, path: &PathBuf) -> anyhow::Result<()> {
    let uri = glib::filename_to_uri(path, None)
        .map_err(|e| anyhow::anyhow!("パスを URI にできません: {e}"))?;
    let doc = poppler::Document::from_file(&uri, None)
        .map_err(|e| anyhow::anyhow!("PDF が開けません: {e}"))?;
    if doc.n_pages() == 0 {
        anyhow::bail!("ページがありません");
    }

    let colors = crate::reader::config::load_colors();
    let (state, warning) = ReaderState::open(path.clone(), colors)?;
    let state = Rc::new(RefCell::new(state));

    // サイドバーはページより後に作るので、あとから差し込む
    let sidebar_slot: Rc<RefCell<Option<Rc<sidebar::Sidebar>>>> = Rc::new(RefCell::new(None));
    let on_created: Rc<dyn Fn(&str)> = {
        let slot = sidebar_slot.clone();
        Rc::new(move |id: &str| {
            // borrow を持ったまま refresh しない
            let sb = slot.borrow().clone();
            let Some(sb) = sb else { return };
            sb.refresh();
            // 空文字は「一覧を作り直すだけ」(削除された直後など)
            if !id.is_empty() {
                sb.focus_memo(id);
            }
        })
    };
    let view = Rc::new(pages::PageView::new(&doc, PAGE_GAP, state.clone(), on_created)?);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(view.widget()));

    let page_label = gtk::Label::new(Some(&format!("p.1/{}", doc.n_pages())));
    let zoom_label = gtk::Label::new(Some("100%"));

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(6);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);

    let zoom_out = gtk::Button::with_label("−");
    let zoom_in = gtk::Button::with_label("＋");
    let fit = gtk::Button::with_label("幅合わせ");
    toolbar.append(&page_label);
    toolbar.append(&zoom_out);
    toolbar.append(&zoom_label);
    toolbar.append(&zoom_in);
    toolbar.append(&fit);

    // 本文の全文検索。走査の中身は install_search / wire_search_keys で後から繋ぐ
    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some("本文を検索 (Ctrl+F)"));
    search_entry.set_hexpand(true);
    search_entry.set_halign(gtk::Align::End);
    search_entry.set_width_chars(20);
    toolbar.append(&search_entry);

    let apply_zoom = {
        let view = view.clone();
        let zoom_label = zoom_label.clone();
        Rc::new(move |z: f64| {
            view.set_zoom(z);
            zoom_label.set_text(&format!("{:.0}%", view.zoom() * 100.0));
        })
    };

    {
        let apply = apply_zoom.clone();
        let view = view.clone();
        zoom_in.connect_clicked(move |_| apply(view.zoom() * 1.25));
    }
    {
        let apply = apply_zoom.clone();
        let view = view.clone();
        zoom_out.connect_clicked(move |_| apply(view.zoom() / 1.25));
    }
    {
        let apply = apply_zoom.clone();
        let view = view.clone();
        let scrolled = scrolled.clone();
        fit.connect_clicked(move |_| {
            let (w, _) = view.page_sizes()[0];
            apply(geom::fit_width_scale(w, scrolled.width() as f64));
        });
    }

    // スクロールに合わせてページ番号を更新する
    {
        let view = view.clone();
        let page_label = page_label.clone();
        let total = doc.n_pages();
        let vadj = scrolled.vadjustment();
        vadj.connect_value_changed(move |adj| {
            let offsets = geom::page_offsets(&view.scaled_heights(), PAGE_GAP);
            // container の set_margin_top(MARGIN) 分、ページ 0 の上端はスクロール座標 MARGIN にある
            let n = geom::visible_page(adj.value() - geom::MARGIN, &offsets);
            page_label.set_text(&format!("p.{}/{}", n + 1, total));
        });
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // 警告バーは常に用意しておく。保存に失敗したときもここに出す
    let warn_bar = gtk::Label::new(None);
    warn_bar.add_css_class("reader-warning");
    warn_bar.set_wrap(true);
    warn_bar.set_xalign(0.0);
    warn_bar.set_visible(false);
    if let Some(msg) = warning {
        warn_bar.set_text(&format!(
            "注釈ファイルが読めないため読み取り専用で開いています。\
             新しいマーカーやメモは保存されません ({msg})"
        ));
        warn_bar.set_visible(true);
    }
    root.append(&warn_bar);
    state.borrow_mut().set_warn_bar(warn_bar);

    // 一覧の行から本文へ飛ぶ。サイドバーがページ側を強く掴むと循環参照になるので弱参照で持つ
    let on_jump: Rc<dyn Fn(usize)> = {
        let view = Rc::downgrade(&view);
        let scrolled = scrolled.downgrade();
        Rc::new(move |page: usize| {
            let (Some(view), Some(scrolled)) = (view.upgrade(), scrolled.upgrade()) else {
                return;
            };
            let offsets = geom::page_offsets(&view.scaled_heights(), PAGE_GAP);
            if let Some(y) = offsets.get(page) {
                // container の set_margin_top(MARGIN) 分だけページ 0 の上端がずれている
                scrolled.vadjustment().set_value(*y + geom::MARGIN);
            }
        })
    };
    // 一覧から注釈を消したときに残った塗りを消す。どのページの行かは分からないので全ページ
    let redraw: Rc<dyn Fn()> = {
        let view = Rc::downgrade(&view);
        Rc::new(move || {
            if let Some(view) = view.upgrade() {
                view.queue_draw_all();
            }
        })
    };
    // outline::read は総ページ数を見ずに /Dest を数値化するだけなので、壊れた・
    // 悪意ある PDF では範囲外のページ番号が返り得る (outline.rs のドキュメント参照)。
    // ジャンプする前に総ページ数でクランプし、範囲外は「飛べない」扱いにする
    let total_pages = usize::try_from(doc.n_pages()).unwrap_or(0);
    let outline_items = clamp_outline(crate::reader::outline::read(&doc), total_pages);
    let sidebar = sidebar::Sidebar::new(state.clone(), on_jump, redraw, outline_items);
    *sidebar_slot.borrow_mut() = Some(sidebar.clone());
    install_search(&doc, sidebar.search(), &view);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.append(&toolbar);
    right.append(&scrolled);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(sidebar.widget()));
    paned.set_end_child(Some(&right));
    paned.set_position(280);
    paned.set_resize_start_child(false);
    // shrink-start-child の既定値 true のままだと、position をこれより小さくしても
    // (ドラッグでも) start 側が中身の最小幅を割って縮み、目次タブのボタンが
    // ペインの外 (負の x) へクリップされてしまう。false にすると position は
    // 中身由来の最小幅 (タブ数に応じて sidebar/mod.rs が持つ) まで自動的に
    // 切り上げられ、常に全ボタンが x>=0 に収まる
    paned.set_shrink_start_child(false);
    root.append(&paned);

    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "miryam-reader".into());
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .default_width(1200)
        .default_height(800)
        .child(&root)
        .build();
    wire_search_keys(&window, &search_entry, &sidebar);
    window.present();

    // 開いた直後は幅合わせにする。合わせたあとしおりのページへ飛ぶ。
    //
    // しおり側は fit-width の適用より後、かつ GTK のレイアウトが実際に確定してから動く必要が
    // ある。set_zoom はページの set_content_width/height を変えてリサイズをキューするだけで、
    // GtkAdjustment の upper は GTK の次のレイアウトパスまで古いまま。同じコールバックの中で
    // vadjustment().set_value() を呼ぶと、値はまだ小さすぎる upper に対してクランプされ、
    // 0 に落ちてしまう (GtkAdjustment::set_value は失敗を出さず黙ってクランプする)。
    //
    // レイアウトが実際に確定して upper/page_size が更新されたときに飛ぶ vadjustment の
    // `changed` シグナルを待ち、実寸が反映された最初の一回だけしおり位置を適用する。
    //
    // ただし `changed` は必ず飛んでくるとは限らない。set_content_width/height は指定した
    // 整数値が現状と変わらなければ何もせず早期returnし、リサイズをキューしない。fit-width
    // の倍率がちょうど 1.0 前後 (だいたい 1.000〜1.002) に収まり、各ページのピクセル寸法が
    // 直前 (zoom=1.0 の初期レイアウト) と同じ整数に丸まってしまう場合がこれに当たる。この
    // まま無条件にハンドラを繋ぐと `changed` は一生発火せずセッション中ずっと残ってしまい、
    // あとでユーザーがズームや窓のリサイズで `changed` を再発火させたときに「最初の
    // changed」としてゲートを通ってしまい、読んでいた場所から古いしおりへ視点を飛ばして
    // しまう (実際に発生を確認)。
    //
    // 判別には「apply() 前後でページの高さのピクセル値 (h*z as i32 の丸め) が 1 つでも
    // 変わるか」(`geom::resize_queued`) を直接見る。`vadj.upper() > vadj.page_size()`
    // (スクロール可能な実コンテンツが既にあるか) では判別できない (実機で確認済み):
    // 複数ページの文書では、今回のリサイズが未処理でも「他のページの分だけで」既に upper が
    // page_size を超えていることが普通にあり、その状態で `changed` を待たずに飛ぶと、GTK が
    // まだ反映していない古い upper に対してクランプされた中途半端な位置がそのまま確定して
    // しまう (fit-width で大きくズームされるケースで実際に発生を確認: scale=1.44 なのに
    // `upper > page_size` が真になり immediate 側に入ってしまい、クランプ後の値のまま
    // `changed` を二度と待たなくなっていた)。ページ寸法の比較なら、これから起こる
    // (起こらない) リサイズそのものを直接見るので取り違えない。
    //
    // 幅は見ない: ページは縦の `gtk::Box` に高さで並んでいるので、vadjustment (縦スクロール)
    // の `changed` を動かすのは高さの変化だけ。幅だけ整数値が変わっても hadjustment が動く
    // だけで vadjustment はそのまま `gtk_adjustment_configure` され `changed` は飛ばない。
    // ここで幅も見てしまうと、幅だけ変わって高さは変わらないケースで「リサイズが積まれる」
    // と誤判定してハンドラを繋いだまま二度と `changed` が来ず、直そうとしている残件そのものが
    // 再発する (詳しくは `geom::resize_queued` のコメント・テスト参照)。
    //
    // `scrolled2.width()` が 0 (まだ一度もレイアウトされておらず fit-width が 1.0 に落ちて
    // いるだけ) のときは「変わらない」を安全側と判断できないので、必ず `changed` を待つ。
    // フレームクロックがこの idle より先に 1 回走るので通常は 0 にならないはずだが、
    // それを確実にコードで保証できていない以上、ここで前提を明示的にチェックする
    //
    // listen を始めるタイミングも重要: これを fit-width 適用より前に仕込むと、ウィンドウ表示
    // 直後・まだ等倍 (zoom=1.0) のままの初期レイアウトで最初の `changed` が飛んできてしまい
    // (GDK のフレームクロックはこの idle コールバックより優先度が高く先に 1 回走る)、その
    // ときの offsets はまだ fit-width 後の寸法ではないので誤ったスクロール位置を計算して
    // 自分を切り離してしまう。connect は apply() の後、この idle コールバックの中で行う。
    //
    // さらに、`changed` ハンドラの中で直接 vadjustment().set_value() を呼んではいけない
    // (実機で確認済み): このハンドラはまさにいま処理中のリサイズの途中で飛んでくるので、
    // ここで set_value しても GTK 側がまだ続けているそのリサイズの残りの処理 (ビューポート
    // 自身のスクロール位置維持ロジック) に、`value-changed` を発火させないまま上書きされて
    // 消える。プロパティの読み出しは変更後の値を返す (借用は壊れていないように見える) のに、
    // 実際に描画される内容は追従しない。呼び出しスタックを抜けてから
    // (`changed` の中で `glib::idle_add_local_once` により次の 1 ターン後に) 設定することで
    // GTK 自身の後処理が終わってから安全に反映できる
    let apply = apply_zoom.clone();
    let view2 = view.clone();
    let scrolled2 = scrolled.clone();
    let bookmark = state.borrow().sidecar.bookmark_page;
    glib::idle_add_local_once(move || {
        let (w, _) = view2.page_sizes()[0];
        let viewport_w = scrolled2.width();
        let scale = geom::fit_width_scale(w, viewport_w as f64);
        let old_zoom = view2.zoom();
        apply(scale);

        if bookmark == 0 {
            return;
        }

        let will_resize = geom::resize_queued(&view2.page_sizes(), old_zoom, scale);
        if viewport_w > 0 && !will_resize {
            schedule_bookmark_scroll(&view2, &scrolled2, bookmark);
            return;
        }

        let vadj = scrolled2.vadjustment();
        // ScrolledWindow (延いては vadjustment) が持つハンドラなので、view/scrolled を強参照で
        // 掴むと ScrolledWindow → vadjustment → ハンドラ → scrolled の循環になる。弱参照で持つ
        let view3 = Rc::downgrade(&view2);
        let scrolled3 = scrolled2.downgrade();
        let handler: Rc<Cell<Option<glib::SignalHandlerId>>> = Rc::new(Cell::new(None));
        let handler_for_closure = handler.clone();
        let id = vadj.connect_changed(move |adj| {
            // ゲートの結果に関わらず、最初の changed で無条件に disarm する。connect は
            // apply() の後で行っているので、これが最初の changed であれば「まだレイアウト
            // 途中」ではなく「これが確定形」。幅合わせの結果ドキュメント全体がビューポートに
            // 収まる (ページが短い/窓が縦に大きい) 場合はこの先のゲートを一度も通らないが、
            // それは単にスクロールする先が無いだけで、レイアウトは確定している。ここで
            // disconnect せずに return すると、ハンドラがセッション中ずっと生き残り、あとで
            // ユーザーがズームや窓のリサイズで changed を再発火させたときにゲートを通って
            // しまい、読んでいた場所から古いしおりへ視点を飛ばしてしまう (実際に発生を確認)
            let Some(id) = handler_for_closure.take() else {
                return;
            };
            adj.disconnect(id);

            // upper が可視領域とほぼ同じ = ドキュメント全体が収まっていてスクロール不要
            if adj.upper() <= adj.page_size() + 1.0 {
                return;
            }
            let (Some(view), Some(scrolled)) = (view3.upgrade(), scrolled3.upgrade()) else {
                return;
            };
            schedule_bookmark_scroll(&view, &scrolled, bookmark);
        });
        handler.set(Some(id));
    });

    // 閉じるときにしおりと最終閲覧日時を書き、マスコットへ読了を知らせる
    {
        let state = state.clone();
        let view = view.clone();
        let scrolled = scrolled.clone();
        window.connect_close_request(move |_| {
            let offsets = geom::page_offsets(&view.scaled_heights(), PAGE_GAP);
            let adj = scrolled.vadjustment();
            // 末尾ではクランプにより value が offsets の最終ページ開始点まで届かないので、
            // visible_page だけだと 1 ページ手前を返す (bookmark_page 関数のコメント参照)。
            // ここでの判定は「upper - page_size にほぼ達しているか」で、しおり復元側の
            // ゲート (`upper <= page_size + 1.0`) と対になる考え方
            let at_end = adj.value() >= adj.upper() - adj.page_size() - 1.0;
            let page = geom::bookmark_page(adj.value() - geom::MARGIN, &offsets, at_end);
            let failed = {
                let mut st = state.borrow_mut();
                st.sidecar.bookmark_page = page;
                st.sidecar.opened_at = chrono::Local::now();
                st.save();
                st.save_failed()
            };
            let name = state
                .borrow()
                .pdf_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let marks = state.borrow().sidecar.highlights.len();
            // 書けていないのに「読みました」とは言わない。窓はこのまま閉じる
            let message = if failed {
                format!("『{name}』のしおりを保存できませんでした")
            } else {
                format!(
                    "『{name}』を {} ページまで読みました (マーカー {marks} 件)",
                    page + 1
                )
            };
            notify_miryam(&message);
            glib::Propagation::Proceed
        });
    }

    Ok(())
}

/// GVariant テキスト形式の文字列リテラル用にエスケープする
pub fn gvariant_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n")
}

/// 起動中の miryam に喋らせる。いなければ黙って諦める
pub fn notify_miryam(message: &str) {
    let arg = format!("[<'{}'>]", gvariant_escape(message));
    let argv: [&std::ffi::OsStr; 14] = [
        "gdbus".as_ref(),
        "call".as_ref(),
        "--session".as_ref(),
        "--dest".as_ref(),
        "dev.youknow.miryam".as_ref(),
        "--object-path".as_ref(),
        "/dev/youknow/miryam".as_ref(),
        "--method".as_ref(),
        "org.gtk.Actions.Activate".as_ref(),
        "say".as_ref(),
        arg.as_ref(),
        "{}".as_ref(),
        // 窓が閉じないより届かないほうがましなので、呼び出し自体に短い上限を持たせる
        "--timeout".as_ref(),
        "2".as_ref(),
    ];
    // 起動していないときの ServiceUnknown は想定内なので stderr は捨てる
    match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDERR_SILENCE) {
        Ok(proc) => {
            let _ = proc.wait(None::<&gio::Cancellable>);
        }
        Err(e) => eprintln!("miryam-reader: gdbus を起動できません: {e}"),
    }
}

fn show_fatal(app: &gtk::Application, msg: &str) {
    let dialog = gtk::MessageDialog::builder()
        .application(app)
        .message_type(gtk::MessageType::Error)
        .buttons(gtk::ButtonsType::Close)
        .text("PDF を開けませんでした")
        .secondary_text(msg)
        .build();
    let app = app.clone();
    dialog.connect_response(move |d, _| {
        d.close();
        app.quit();
    });
    dialog.present();
}

/// 画面を出さずに確かめられるのは状態の側だけ。ここでは GTK を触らない
/// (`ReaderState` は `warn_bar` を持つが、`None` のままなら初期化は要らない)
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// tempdir に空でない PDF もどきを作る。中身は読まないので実体は何でもよい
    fn fixture(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("foo.pdf");
        let mut f = std::fs::File::create(&path).expect("作成できること");
        f.write_all(b"%PDF-1.7\n").expect("書けること");
        path
    }

    /// 開いてハイライトを 1 件足した状態にする (この時点で sidecar は保存済み)
    fn opened_with_one(pdf: &std::path::Path) -> (ReaderState, String) {
        let (mut state, warning) =
            ReaderState::open(pdf.to_path_buf(), vec!["yellow".into()]).expect("開けること");
        assert!(warning.is_none(), "新規なら警告は出ない");
        let id = state.add_highlight(0, "yellow", vec![[0.1, 0.1, 0.2, 0.2]], "引用".into());
        (state, id)
    }

    #[test]
    fn set_memo_writes_the_new_text_to_the_sidecar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        let (mut state, id) = opened_with_one(&pdf);

        state.set_memo(&id, "あとで読み返す".into());

        let loaded = store::Sidecar::load(&pdf).expect("読めること").expect("ある");
        assert_eq!(loaded.highlights[0].memo, "あとで読み返す");
        assert!(state.save_error.is_none(), "保存に失敗していない");
    }

    #[test]
    fn set_memo_with_the_same_text_does_not_save_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        let (mut state, id) = opened_with_one(&pdf);
        state.set_memo(&id, "同じ文".into());

        // 保存が走ったら必ず上書きされる印を置いておく
        let path = store::sidecar_path(&pdf);
        std::fs::write(&path, "書き換えられたら消える印").expect("書けること");

        state.set_memo(&id, "同じ文".into());

        let after = std::fs::read_to_string(&path).expect("読めること");
        assert_eq!(after, "書き換えられたら消える印", "同じ値なら保存しない");
    }

    #[test]
    fn set_memo_with_an_unknown_id_changes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        let (mut state, _id) = opened_with_one(&pdf);

        let path = store::sidecar_path(&pdf);
        std::fs::write(&path, "書き換えられたら消える印").expect("書けること");

        state.set_memo("そんな id は無い", "メモ".into());

        let after = std::fs::read_to_string(&path).expect("読めること");
        assert_eq!(after, "書き換えられたら消える印", "知らない id では保存しない");
        assert_eq!(state.sidecar.highlights[0].memo, "", "メモも増えない");
    }

    /// 壊れたサイドカーを置いて、その中身を返す (バイト比較の基準にする)
    fn put_broken_sidecar(pdf: &std::path::Path) -> Vec<u8> {
        let path = store::sidecar_path(pdf);
        std::fs::create_dir_all(path.parent().expect("親がある")).expect("掘れること");
        let broken = b"{ \x82\xa0 broken".to_vec();
        std::fs::write(&path, &broken).expect("書けること");
        broken
    }

    #[test]
    fn a_broken_sidecar_opens_read_only_with_a_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        put_broken_sidecar(&pdf);

        let (state, warning) =
            ReaderState::open(pdf.clone(), vec!["yellow".into()]).expect("開けること");

        assert!(state.read_only, "壊れていたら読み取り専用");
        let warning = warning.expect("警告文が返る");
        assert!(!warning.is_empty(), "理由が入っている");
    }

    #[test]
    fn read_only_never_touches_the_broken_file_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        let before = put_broken_sidecar(&pdf);

        let (mut state, _warning) =
            ReaderState::open(pdf.clone(), vec!["yellow".into()]).expect("開けること");
        assert!(state.read_only, "前提: 読み取り専用");

        let id = state.add_highlight(0, "yellow", vec![[0.1, 0.1, 0.2, 0.2]], "引用".into());
        state.set_memo(&id, "メモ".into());
        state.sidecar.bookmark_page = 7;
        state.save();

        let after = std::fs::read(store::sidecar_path(&pdf)).expect("読めること");
        assert_eq!(after, before, "読み取り専用ではユーザーの記録を 1 バイトも書き換えない");
        assert!(!state.save_failed(), "書きに行かないので失敗も記録しない");
    }

    #[test]
    fn remove_highlight_roundtrips_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        let (mut state, id) = opened_with_one(&pdf);
        let saved = store::Sidecar::load(&pdf).expect("読めること").expect("ある");
        assert_eq!(saved.highlights.len(), 1, "前提: 1 件保存されている");

        state.remove_highlight(&id);

        let loaded = store::Sidecar::load(&pdf).expect("読めること").expect("ある");
        assert!(loaded.highlights.is_empty(), "消したらファイルからも消える");
        assert!(state.sidecar.highlights.is_empty(), "手元の状態からも消える");
        assert!(!state.save_failed(), "保存に失敗していない");
    }

    #[test]
    fn a_successful_save_clears_the_recorded_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path());
        let (mut state, id) = opened_with_one(&pdf);

        // 保存先を書けなくして 1 回失敗させる (ディレクトリを消す)
        let sidecar_dir = store::sidecar_path(&pdf).parent().expect("親がある").to_path_buf();
        std::fs::remove_dir_all(&sidecar_dir).expect("消せること");
        std::fs::write(&sidecar_dir, "dir ではなくファイル").expect("書けること");
        state.set_memo(&id, "失敗するメモ".into());
        assert!(state.save_failed(), "前提: 保存に失敗している");

        // 邪魔を取り除けば次の保存は通る
        std::fs::remove_file(&sidecar_dir).expect("消せること");
        state.set_memo(&id, "通るメモ".into());

        assert!(!state.save_failed(), "成功したら失敗の記録は消える");
        let loaded = store::Sidecar::load(&pdf).expect("読めること").expect("ある");
        assert_eq!(loaded.highlights[0].memo, "通るメモ");
    }

    #[test]
    fn escapes_backslash_quote_and_newline() {
        assert_eq!(gvariant_escape(r"a\b"), r"a\\b");
        assert_eq!(gvariant_escape("it's"), r"it\'s");
        assert_eq!(gvariant_escape("a\nb"), r"a\nb");
    }

    #[test]
    fn escape_order_does_not_double_escape() {
        // バックスラッシュを先に処理するので、後から入る \' が壊れない
        assert_eq!(gvariant_escape(r"\'"), r"\\\'");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(gvariant_escape("『本』を読みました"), "『本』を読みました");
    }

    fn outline_item(
        title: &str,
        page: Option<usize>,
        children: Vec<crate::reader::outline::OutlineItem>,
    ) -> crate::reader::outline::OutlineItem {
        crate::reader::outline::OutlineItem { title: title.to_string(), page, children }
    }

    #[test]
    fn clamp_outline_drops_a_page_beyond_total_pages() {
        let items = vec![outline_item("A", Some(9998), vec![])];

        let clamped = clamp_outline(items, 2);

        assert_eq!(clamped[0].page, None, "総ページ数を超えた飛び先は飛べない扱いにする");
    }

    #[test]
    fn clamp_outline_rejects_the_exact_boundary_but_keeps_one_below() {
        let total_pages = 5;
        let items = vec![
            outline_item("最終ページ", Some(total_pages - 1), vec![]),
            outline_item("範囲外", Some(total_pages), vec![]),
        ];

        let clamped = clamp_outline(items, total_pages);

        assert_eq!(
            clamped[0].page,
            Some(total_pages - 1),
            "p == total_pages - 1 は文書内の最終ページなので生き残ること"
        );
        assert_eq!(
            clamped[1].page, None,
            "p == total_pages はその文書には存在しないページなので弾くこと"
        );
    }

    #[test]
    fn clamp_outline_reaches_into_children() {
        let items = vec![outline_item(
            "親",
            Some(0),
            vec![outline_item("子", Some(9998), vec![])],
        )];

        let clamped = clamp_outline(items, 2);

        assert_eq!(
            clamped[0].children[0].page, None,
            "子項目の飛び先も総ページ数でクランプされること"
        );
    }
}
