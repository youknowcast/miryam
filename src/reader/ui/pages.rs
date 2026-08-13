use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::reader::geom;
use crate::reader::store;
use crate::reader::ui::ReaderState;

/// ページを縦に並べたビュー。各ページ 1 枚の DrawingArea を持つ
pub struct PageView {
    container: gtk::Box,
    areas: Vec<gtk::DrawingArea>,
    /// (幅, 高さ) をポイント単位で
    sizes: Vec<(f64, f64)>,
    zoom: Rc<Cell<f64>>,
}

impl PageView {
    pub fn new(
        doc: &poppler::Document,
        gap: f64,
        state: Rc<RefCell<ReaderState>>,
        on_created: Rc<dyn Fn(&str)>,
    ) -> anyhow::Result<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, gap as i32);
        container.set_halign(gtk::Align::Center);
        container.set_margin_top(geom::MARGIN as i32);
        container.set_margin_bottom(geom::MARGIN as i32);

        let zoom = Rc::new(Cell::new(1.0_f64));
        let mut areas = Vec::new();
        let mut sizes = Vec::new();

        // 変数名は後続タスク (選択・ハイライト描画) がそのまま使うので、この名前を守る
        for i in 0..doc.n_pages() {
            let index = i as usize;
            let page = doc
                .page(i)
                .ok_or_else(|| anyhow::anyhow!("{} ページ目が読めません", i + 1))?;
            let (page_w, page_h) = page.size();
            sizes.push((page_w, page_h));

            let area = gtk::DrawingArea::new();
            area.set_content_width((page_w * zoom.get()) as i32);
            area.set_content_height((page_h * zoom.get()) as i32);
            area.add_css_class("reader-page");

            // 断り書きを重ねるための入れ物。中身は必要になってから足す
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&area));

            // 文字情報の有無。page.text() は全文抽出で重いので、実際に描かれたときに 1 回だけ調べる
            let has_text: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));

            // GObject の clone は参照カウント増加。選択処理でも同じページを使う
            let page_for_draw = page.clone();
            let zoom_for_draw = zoom.clone();
            let state_for_draw = state.clone();
            let has_text_for_draw = has_text.clone();
            // 強参照だと overlay → area → draw クロージャ → overlay の循環になる
            let overlay_for_draw = overlay.downgrade();
            area.set_draw_func(move |_area, cr, _w, _h| {
                if has_text_for_draw.get().is_none() {
                    let found = page_for_draw.text().is_some_and(|t| !t.trim().is_empty());
                    has_text_for_draw.set(Some(found));
                    if !found
                        && let Some(ov) = overlay_for_draw.upgrade()
                    {
                        // 描画の最中にウィジェットを足さない
                        gtk::glib::idle_add_local_once(move || attach_no_text_note(&ov));
                    }
                }
                let z = zoom_for_draw.get();
                // 紙の白
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.paint();
                let _ = cr.save();
                cr.scale(z, z);
                page_for_draw.render(cr);

                // 保存済みハイライトを半透明で重ねる
                {
                    let st = state_for_draw.borrow();
                    for h in st.sidecar.highlights.iter().filter(|h| h.page == index) {
                        let (r, g, b) = color_rgb(&h.color);
                        cr.set_source_rgba(r, g, b, 0.35);
                        for rect in &h.rects {
                            let (x0, y0, x1, y1) = store::denormalize_rect(rect, page_w, page_h);
                            cr.rectangle(x0, y0, x1 - x0, y1 - y0);
                        }
                        let _ = cr.fill();
                    }
                }

                let _ = cr.restore();
            });

            // 開いているポップオーバーの置き場。窓が壊れるときにここから外す
            let popover_slot: Rc<RefCell<Option<gtk::Popover>>> = Rc::new(RefCell::new(None));
            {
                let slot = popover_slot.clone();
                area.connect_destroy(move |_| {
                    let open = slot.borrow_mut().take();
                    if let Some(p) = open
                        && p.parent().is_some()
                    {
                        p.unparent();
                    }
                });
            }

            // ドラッグで本文を選び、離したところで色を選ばせる
            let page_for_drag = page.clone();
            let on_created_for_drag = on_created.clone();
            let drag = gtk::GestureDrag::new();
            let selection: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
            {
                let selection = selection.clone();
                drag.connect_drag_begin(move |_, x, y| selection.set(Some((x, y))));
            }
            {
                let selection = selection.clone();
                let zoom = zoom.clone();
                let page = page_for_drag;
                let state = state.clone();
                let area_for_popover = area.clone();
                let slot = popover_slot.clone();
                drag.connect_drag_end(move |_, dx, dy| {
                    let Some((sx, sy)) = selection.replace(None) else {
                        return;
                    };
                    let z = zoom.get();
                    // ウィジェット座標 → ページ座標
                    let (x0, y0) = (sx / z, sy / z);
                    let (x1, y1) = ((sx + dx) / z, (sy + dy) / z);
                    if (x1 - x0).abs() < 2.0 && (y1 - y0).abs() < 2.0 {
                        return; // クリック扱い。選択にはしない
                    }
                    let mut rect = poppler::Rectangle::default();
                    rect.set_x1(x0.min(x1));
                    rect.set_y1(y0.min(y1));
                    rect.set_x2(x0.max(x1));
                    rect.set_y2(y0.max(y1));

                    let quote = page
                        .selected_text(poppler::SelectionStyle::Glyph, &mut rect)
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    if quote.trim().is_empty() {
                        return; // 文字情報が無い or 空選択
                    }
                    let Some(region) =
                        page.selected_region(1.0, poppler::SelectionStyle::Glyph, &mut rect)
                    else {
                        return;
                    };
                    let (pw, ph) = page.size();
                    let mut rects = Vec::new();
                    for i in 0..region.num_rectangles() {
                        let r = region.rectangle(i);
                        rects.push(store::normalize_rect(
                            r.x() as f64,
                            r.y() as f64,
                            (r.x() + r.width()) as f64,
                            (r.y() + r.height()) as f64,
                            pw,
                            ph,
                        ));
                    }
                    if rects.is_empty() {
                        return;
                    }
                    show_color_popover(
                        &area_for_popover,
                        &slot,
                        &state,
                        &on_created_for_drag,
                        index,
                        rects,
                        quote,
                        sx,
                        sy,
                    );
                });
            }
            area.add_controller(drag);

            // 既存マーカーのクリックで編集ポップオーバーを出す。
            // ドラッグと取り違えないよう、押した点からほとんど動いていないときだけ反応する
            let click = gtk::GestureClick::new();
            let press_at: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
            {
                let press_at = press_at.clone();
                click.connect_pressed(move |_, _, x, y| press_at.set(Some((x, y))));
            }
            {
                let press_at = press_at.clone();
                click.connect_cancel(move |_, _| press_at.set(None));
            }
            {
                let state = state.clone();
                let zoom = zoom.clone();
                let slot = popover_slot.clone();
                let on_created = on_created.clone();
                // 強参照にすると area → コントローラ → クロージャ → area の循環になる
                let area_weak = area.downgrade();
                let (pw, ph) = (page_w, page_h);
                click.connect_released(move |_, _, x, y| {
                    let Some((px, py)) = press_at.replace(None) else {
                        return;
                    };
                    let z = zoom.get();
                    // しきい値はドラッグ側と同じページ座標で測る。
                    // ウィジェット座標のまま比べると等倍のときしか排他にならない
                    if (x - px).abs() / z >= 2.0 || (y - py).abs() / z >= 2.0 {
                        return; // ドラッグ扱い。選択側に任せる
                    }
                    // ウィジェット座標 → 正規化座標
                    let (nx, ny) = (x / z / pw, y / z / ph);
                    // 重なっていたら後から引いたもの (上に描かれている) を拾う
                    let hit = state
                        .borrow()
                        .sidecar
                        .highlights
                        .iter()
                        .rev()
                        .find(|h| h.page == index && geom::hit_test(&h.rects, nx, ny))
                        .map(|h| h.id.clone());
                    let (Some(id), Some(area)) = (hit, area_weak.upgrade()) else {
                        return;
                    };
                    show_edit_popover(&area, &slot, &state, &on_created, &id, x, y);
                });
            }
            area.add_controller(click);

            container.append(&overlay);
            areas.push(area);
        }

        Ok(Self { container, areas, sizes, zoom })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    pub fn page_sizes(&self) -> Vec<(f64, f64)> {
        self.sizes.clone()
    }

    pub fn zoom(&self) -> f64 {
        self.zoom.get()
    }

    /// 全ページを描き直す。どのページの注釈が変わったか分からないとき (サイドバーからの削除) 用
    pub fn queue_draw_all(&self) {
        for area in &self.areas {
            area.queue_draw();
        }
    }

    pub fn set_zoom(&self, z: f64) {
        let z = geom::clamp_zoom(z);
        self.zoom.set(z);
        for (area, (w, h)) in self.areas.iter().zip(&self.sizes) {
            area.set_content_width((w * z) as i32);
            area.set_content_height((h * z) as i32);
            area.queue_draw();
        }
    }

    /// 各ページの高さ (現在のズーム適用後)。ウィジェットの `set_content_height` と
    /// 同じ丸め (切り捨て) を揃えて、`geom::page_offsets` が実際のレイアウトとずれないようにする
    pub fn scaled_heights(&self) -> Vec<f64> {
        let z = self.zoom.get();
        self.sizes.iter().map(|(_, h)| (h * z).trunc()).collect()
    }
}

/// 色名 → RGB (0.0〜1.0)
pub fn color_rgb(name: &str) -> (f64, f64, f64) {
    match name {
        "green" => (0.45, 0.85, 0.45),
        "blue" => (0.45, 0.65, 0.95),
        "pink" => (0.98, 0.55, 0.75),
        _ => (0.98, 0.90, 0.35),
    }
}

/// 文字情報のないページに出す断り書き。
/// cairo の toy font は字形の代替が効かず日本語が豆腐になるので Pango を使う Label にする
fn attach_no_text_note(overlay: &gtk::Overlay) {
    let note = gtk::Label::new(None);
    note.set_markup("<span foreground=\"#555555\">このページは文字情報がないため選択できません</span>");
    note.set_halign(gtk::Align::Start);
    note.set_valign(gtk::Align::Start);
    note.set_margin_start(8);
    note.set_margin_top(4);
    // ページのドラッグを邪魔しない
    note.set_can_target(false);
    overlay.add_overlay(&note);
}

/// ページ上の 1 点にポップオーバーを開く。開けるのは同時に 1 つだけ
fn new_popover(
    anchor: &gtk::DrawingArea,
    slot: &Rc<RefCell<Option<gtk::Popover>>>,
    x: f64,
    y: f64,
) -> gtk::Popover {
    // 前のポップオーバーが開いていたら先に片づける (borrow を持ったまま popdown しない)
    let previous = slot.borrow_mut().take();
    if let Some(p) = previous {
        p.popdown();
        if p.parent().is_some() {
            p.unparent();
        }
    }

    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(true);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    // set_parent した Popover は閉じたら自分で外す。放っておくと親に残り続ける
    {
        let slot = slot.clone();
        popover.connect_closed(move |p| {
            let _ = slot.borrow_mut().take();
            if p.parent().is_some() {
                p.unparent();
            }
        });
    }
    popover
}

/// 読み取り専用のときに理由だけを出す中身
fn read_only_note(text: &str) -> gtk::Label {
    let msg = gtk::Label::new(Some(text));
    msg.set_margin_top(4);
    msg.set_margin_bottom(4);
    msg.set_margin_start(6);
    msg.set_margin_end(6);
    msg
}

/// 既存マーカーをクリックしたときのポップオーバー (メモへ移動 / 削除)。
/// 読み取り専用のときはどちらも出さず、その理由を出す
fn show_edit_popover(
    anchor: &gtk::DrawingArea,
    slot: &Rc<RefCell<Option<gtk::Popover>>>,
    state: &Rc<RefCell<ReaderState>>,
    on_changed: &Rc<dyn Fn(&str)>,
    id: &str,
    x: f64,
    y: f64,
) {
    let popover = new_popover(anchor, slot, x, y);

    let read_only = state.borrow().read_only;
    if read_only {
        // 現状は到達しない (読み取り専用のとき highlights は必ず空で、当たり判定が空振りする)。
        // 将来 読み取り専用でも注釈を持てるようになったときのための一貫した扱い
        popover.set_child(Some(&read_only_note("読み取り専用のため編集できません")));
    } else {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let memo = gtk::Button::with_label("メモを書く");
        let delete = gtk::Button::with_label("削除");
        row.append(&memo);
        row.append(&delete);
        popover.set_child(Some(&row));

        {
            let on_changed = on_changed.clone();
            let id = id.to_string();
            // 強参照だと popover → row → button → クロージャ → popover の循環になる
            let popover_weak = popover.downgrade();
            memo.connect_clicked(move |_| {
                // 先に閉じる。開いたままフォーカスを移すとポップオーバーに奪い返される
                if let Some(p) = popover_weak.upgrade() {
                    p.popdown();
                }
                on_changed(&id);
            });
        }
        {
            let state = state.clone();
            let on_changed = on_changed.clone();
            let id = id.to_string();
            let anchor_weak = anchor.downgrade();
            let popover_weak = popover.downgrade();
            delete.connect_clicked(move |_| {
                state.borrow_mut().remove_highlight(&id);
                // 削除の保存が失敗したことを黙って捨てない (state を借りていない状態で呼ぶ)
                ReaderState::show_save_error(&state);
                if let Some(a) = anchor_weak.upgrade() {
                    a.queue_draw();
                }
                // 空文字は「一覧を作り直すだけ」
                on_changed("");
                if let Some(p) = popover_weak.upgrade() {
                    p.popdown();
                }
            });
        }
    }

    slot.replace(Some(popover.clone()));
    popover.popup();
}

/// 選択直後に出すポップオーバー。読み取り専用のときは色を出さず、その理由を出す
fn show_color_popover(
    anchor: &gtk::DrawingArea,
    slot: &Rc<RefCell<Option<gtk::Popover>>>,
    state: &Rc<RefCell<ReaderState>>,
    on_created: &Rc<dyn Fn(&str)>,
    page: usize,
    rects: Vec<[f64; 4]>,
    quote: String,
    x: f64,
    y: f64,
) {
    let popover = new_popover(anchor, slot, x, y);

    let read_only = state.borrow().read_only;
    if read_only {
        popover.set_child(Some(&read_only_note("読み取り専用のためマーカーを作れません")));
    } else {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let colors = state.borrow().colors.clone();
        // 色ボタンは 1 度だけ効く。2 度押しでハイライトが増えないよう take() で取り出す
        let shared = Rc::new(RefCell::new(Some((rects, quote))));
        for color in colors {
            let button = gtk::Button::new();
            button.set_tooltip_text(Some(&color));
            let swatch = gtk::DrawingArea::new();
            swatch.set_content_width(20);
            swatch.set_content_height(20);
            let c = color.clone();
            swatch.set_draw_func(move |_, cr, w, h| {
                let (r, g, b) = color_rgb(&c);
                cr.set_source_rgb(r, g, b);
                cr.rectangle(0.0, 0.0, w as f64, h as f64);
                let _ = cr.fill();
            });
            button.set_child(Some(&swatch));

            let state = state.clone();
            let on_created = on_created.clone();
            let shared = shared.clone();
            // 強参照だと popover → row → button → クロージャ → popover の循環になり、
            // 選択のたびにポップオーバー一式が residue として残る
            let popover_weak = popover.downgrade();
            let anchor_weak = anchor.downgrade();
            button.connect_clicked(move |_| {
                // borrow は if の外で終わらせる。on_created が何を触っても衝突しないように
                let taken = shared.borrow_mut().take();
                // 先に閉じる。開いたままフォーカスを移すとポップオーバーに奪い返される
                // (shared.take() の一度きりガードがあるので、閉じてから作っても二重にならない)
                if let Some(p) = popover_weak.upgrade() {
                    p.popdown();
                }
                if let Some((rects, quote)) = taken {
                    let id = state.borrow_mut().add_highlight(page, &color, rects, quote);
                    if let Some(a) = anchor_weak.upgrade() {
                        a.queue_draw();
                    }
                    // 保存に失敗していたら警告バーに出す (state を借りていない状態で呼ぶ)
                    ReaderState::show_save_error(&state);
                    on_created(&id);
                }
            });
            row.append(&button);
        }
        popover.set_child(Some(&row));
    }

    slot.replace(Some(popover.clone()));
    popover.popup();
}
