use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::Cell;
use std::rc::Rc;

use crate::reader::geom;

/// ページを縦に並べたビュー。各ページ 1 枚の DrawingArea を持つ
pub struct PageView {
    container: gtk::Box,
    areas: Vec<gtk::DrawingArea>,
    /// (幅, 高さ) をポイント単位で
    sizes: Vec<(f64, f64)>,
    zoom: Rc<Cell<f64>>,
}

impl PageView {
    pub fn new(doc: &poppler::Document, gap: f64) -> Self {
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
            let page = doc.page(i).expect("ページ数の範囲内");
            let (page_w, page_h) = page.size();
            sizes.push((page_w, page_h));

            let area = gtk::DrawingArea::new();
            area.set_content_width((page_w * zoom.get()) as i32);
            area.set_content_height((page_h * zoom.get()) as i32);
            area.add_css_class("reader-page");

            // GObject の clone は参照カウント増加。選択処理でも同じページを使う
            let page_for_draw = page.clone();
            let zoom_for_draw = zoom.clone();
            area.set_draw_func(move |_area, cr, _w, _h| {
                let z = zoom_for_draw.get();
                // 紙の白
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.paint();
                cr.save().expect("save");
                cr.scale(z, z);
                page_for_draw.render(cr);
                cr.restore().expect("restore");
            });

            let _ = index; // 後続タスクで使う
            container.append(&area);
            areas.push(area);
        }

        Self { container, areas, sizes, zoom }
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

    /// 各ページの高さ (現在のズーム適用後)
    pub fn scaled_heights(&self) -> Vec<f64> {
        let z = self.zoom.get();
        self.sizes.iter().map(|(_, h)| h * z).collect()
    }
}
