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

            // 文字情報のないページ (スキャン PDF) は選択できないので断りを描く
            let has_text = page.text().is_some_and(|t| !t.trim().is_empty());

            // GObject の clone は参照カウント増加。選択処理でも同じページを使う
            let page_for_draw = page.clone();
            let zoom_for_draw = zoom.clone();
            let state_for_draw = state.clone();
            area.set_draw_func(move |_area, cr, _w, _h| {
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

            if has_text {
                container.append(&area);
            } else {
                // 断り書きは cairo の toy font だと日本語が豆腐になる (字形の代替が効かない)。
                // Pango を使う Label を重ねて出す
                let overlay = gtk::Overlay::new();
                overlay.set_child(Some(&area));
                let note = gtk::Label::new(None);
                note.set_markup(
                    "<span foreground=\"#555555\">このページは文字情報がないため選択できません</span>",
                );
                note.set_halign(gtk::Align::Start);
                note.set_valign(gtk::Align::Start);
                note.set_margin_start(8);
                note.set_margin_top(4);
                // ページのドラッグを邪魔しない
                note.set_can_target(false);
                overlay.add_overlay(&note);
                container.append(&overlay);
            }
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

/// 選択直後に出す色選びのポップオーバー
fn show_color_popover(
    anchor: &gtk::DrawingArea,
    state: &Rc<RefCell<ReaderState>>,
    on_created: &Rc<dyn Fn(&str)>,
    page: usize,
    rects: Vec<[f64; 4]>,
    quote: String,
    x: f64,
    y: f64,
) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(true);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    // set_parent した Popover は閉じたら自分で外す。放っておくと溜まる
    popover.connect_closed(|p| {
        let p = p.clone();
        gtk::glib::idle_add_local_once(move || {
            if p.parent().is_some() {
                p.unparent();
            }
        });
    });

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
        let popover_for_click = popover.clone();
        let anchor = anchor.clone();
        let shared = shared.clone();
        button.connect_clicked(move |_| {
            // borrow は if の外で終わらせる。on_created が何を触っても衝突しないように
            let taken = shared.borrow_mut().take();
            if let Some((rects, quote)) = taken {
                let id = state.borrow_mut().add_highlight(page, &color, rects, quote);
                anchor.queue_draw();
                on_created(&id);
            }
            popover_for_click.popdown();
        });
        row.append(&button);
    }
    popover.set_child(Some(&row));
    popover.popup();
}
