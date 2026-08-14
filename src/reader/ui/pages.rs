use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::reader::geom;
use crate::reader::search::Hit;
use crate::reader::store;
use crate::reader::ui::ReaderState;

/// ページを縦に並べたビュー。各ページ 1 枚の DrawingArea を持つ
pub struct PageView {
    container: gtk::Box,
    areas: Vec<gtk::DrawingArea>,
    /// (幅, 高さ) をポイント単位で
    sizes: Vec<(f64, f64)>,
    zoom: Rc<Cell<f64>>,
    /// ページごとの検索ヒット (正規化矩形)。`areas` と同じ並び。
    /// 各要素は対応するページの draw クロージャと共有する
    search_hits: Vec<Rc<RefCell<Vec<[f64; 4]>>>>,
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
        let mut search_hits = Vec::new();

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

            // ドラッグ中だけ生きる、コミット前の選択範囲 (ページ座標を正規化した矩形群)。
            // 保存済みハイライトと同じ形なので描画は同じ経路 (denormalize_rect) を使う
            let live_selection: Rc<RefCell<Option<Vec<[f64; 4]>>>> = Rc::new(RefCell::new(None));

            // このページの検索ヒット。`set_search_hits` が書き、draw クロージャが読む。
            // 書き手は idle (走査) だけで、描画中に書き換わることはない
            let page_hits: Rc<RefCell<Vec<[f64; 4]>>> = Rc::new(RefCell::new(Vec::new()));

            // GObject の clone は参照カウント増加。選択処理でも同じページを使う
            let page_for_draw = page.clone();
            let zoom_for_draw = zoom.clone();
            let state_for_draw = state.clone();
            let has_text_for_draw = has_text.clone();
            let live_selection_for_draw = live_selection.clone();
            let page_hits_for_draw = page_hits.clone();
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
                        let (r, g, b) = crate::reader::config::color_rgb(&h.color);
                        cr.set_source_rgba(r, g, b, 0.35);
                        for rect in &h.rects {
                            let (x0, y0, x1, y1) = store::denormalize_rect(rect, page_w, page_h);
                            cr.rectangle(x0, y0, x1 - x0, y1 - y0);
                        }
                        let _ = cr.fill();
                    }
                }

                // ドラッグ中の選択範囲を選択青で重ねる (マーカーの 4 色とは別の色にして、
                // コミット前の下書きだと一目でわかるようにする)
                if let Some(rects) = live_selection_for_draw.borrow().as_ref() {
                    cr.set_source_rgba(0.10, 0.45, 0.95, 0.45);
                    for rect in rects {
                        let (x0, y0, x1, y1) = store::denormalize_rect(rect, page_w, page_h);
                        cr.rectangle(x0, y0, x1 - x0, y1 - y0);
                    }
                    let _ = cr.fill();
                }

                // 検索ヒットを一番上に橙で重ねる (選択青ともマーカーの 4 色とも別の色にして、
                // 「いま検索で当たっているところ」だと一目で分かるようにする)
                {
                    let hits = page_hits_for_draw.borrow();
                    if !hits.is_empty() {
                        cr.set_source_rgba(1.0, 0.55, 0.0, 0.45);
                        for rect in hits.iter() {
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

            // ドラッグで本文を選び、離したところで色を選ばせる。ドラッグ中は live_selection に
            // 実際の poppler 選択領域を入れておき、上の draw_func が選択青で重ねて見せる
            let page_for_drag = page.clone();
            let page_for_drag_update = page.clone();
            let on_created_for_drag = on_created.clone();
            let drag = gtk::GestureDrag::new();
            let selection: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
            {
                let selection = selection.clone();
                drag.connect_drag_begin(move |_, x, y| selection.set(Some((x, y))));
            }
            {
                // シーケンスが cancel されたとき (例: 親の ScrolledWindow のパンジェスチャに
                // グラブを奪われた) は drag-end が来ない可能性がある。その場合でも下書きを
                // 残さない。GTK4 は通常この後 ::end も出すが、それに頼らず無条件で消す
                let selection = selection.clone();
                let live_selection = live_selection.clone();
                let area_weak = area.downgrade();
                drag.connect_cancel(move |_, _| {
                    selection.set(None);
                    if let Some(area) = area_weak.upgrade() {
                        clear_live_selection(&live_selection, &area);
                    }
                });
            }
            {
                let selection = selection.clone();
                let zoom = zoom.clone();
                let page = page_for_drag_update;
                // 強参照だと area → コントローラ → クロージャ → area の循環になる
                let area_weak_for_update = area.downgrade();
                let live_selection = live_selection.clone();
                drag.connect_drag_update(move |_, dx, dy| {
                    let Some((sx, sy)) = selection.get() else {
                        return;
                    };
                    let z = zoom.get();
                    // ウィジェット座標 → ページ座標
                    let (x0, y0) = (sx / z, sy / z);
                    let (x1, y1) = ((sx + dx) / z, (sy + dy) / z);
                    // 実測 (数百語/ページの本文で ~20〜50µs、ページ内最初の 1 回でも ~1ms) は
                    // 動きイベントの予算 (目安 8ms) を大きく下回るので compute_selection 自体は
                    // 間引かない。だが本当のコストは選択計算ではなく毎回のページ全再描画
                    // (DrawingArea に部分再描画は無く、draw_func は毎回 render(cr) からやり直す)
                    // なので、再描画は選択矩形が実際に変わった (ポインタがグリフ境界をまたいだ)
                    // ときだけに絞る。これで単語の中をゆっくり動かす間の再描画も、しきい値未満の
                    // 微動を伴う単なるクリックの再描画も消える
                    let rects = compute_selection(&page, x0, y0, x1, y1).map(|(_, rects)| rects);
                    let changed = {
                        let current = live_selection.borrow();
                        let prev: &[[f64; 4]] = current.as_deref().unwrap_or(&[]);
                        let next: &[[f64; 4]] = rects.as_deref().unwrap_or(&[]);
                        geom::rects_changed(prev, next)
                    };
                    if !changed {
                        return;
                    }
                    *live_selection.borrow_mut() = rects;
                    if let Some(area) = area_weak_for_update.upgrade() {
                        area.queue_draw();
                    }
                });
            }
            {
                let selection = selection.clone();
                let zoom = zoom.clone();
                let page = page_for_drag;
                let state = state.clone();
                let area_for_popover = area.clone();
                let slot = popover_slot.clone();
                let live_selection = live_selection.clone();
                drag.connect_drag_end(move |_, dx, dy| {
                    let Some((sx, sy)) = selection.replace(None) else {
                        // 理論上しか無い経路 (drag-begin 抜きの drag-end) だが、
                        // 早期 return はすべて live_selection を消す方針を徹底する
                        clear_live_selection(&live_selection, &area_for_popover);
                        return;
                    };
                    let z = zoom.get();
                    // ウィジェット座標 → ページ座標
                    let (x0, y0) = (sx / z, sy / z);
                    let (x1, y1) = ((sx + dx) / z, (sy + dy) / z);
                    if (x1 - x0).abs() < 2.0 && (y1 - y0).abs() < 2.0 {
                        // クリック扱い。選択にはしない。drag_update で入った下書きも消しておく
                        clear_live_selection(&live_selection, &area_for_popover);
                        return;
                    }
                    let Some((quote, rects)) = compute_selection(&page, x0, y0, x1, y1) else {
                        clear_live_selection(&live_selection, &area_for_popover);
                        return; // 文字情報が無い / 空選択 / 選択領域が空
                    };
                    // ここでは live_selection をまだ消さない。色ポップオーバーが開いている間も
                    // 選択範囲を見せ続け、閉じたとき (色を選んでも Esc で消しても) に消す
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
                        live_selection.clone(),
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
            search_hits.push(page_hits);
        }

        Ok(Self { container, areas, sizes, zoom, search_hits })
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

    /// 検索ヒットの強調を入れ替える。**空の `hits` を渡すと全ページの強調が消える**
    ///
    /// 走査は少しずつ進むので、これは 1 チャンクごとに「そこまでに見つかった全部」で
    /// 呼ばれる。毎回全ページを描き直すと重いので、**矩形集合が実際に変わった
    /// ページだけ** `queue_draw` する (DrawingArea に部分再描画は無く、描き直しは
    /// `render(cr)` からのやり直しになるため。`geom::rects_changed` の用途は
    /// ドラッグ選択の抑制と同じ)
    pub fn set_search_hits(&self, hits: Vec<Hit>) {
        let mut next: Vec<Vec<[f64; 4]>> = vec![Vec::new(); self.areas.len()];
        for hit in &hits {
            // 範囲外のページ番号は捨てる (走査は総ページ数の中を歩くので通常は無い)
            if let Some(slot) = next.get_mut(hit.page) {
                slot.extend_from_slice(&hit.rects);
            }
        }
        for (i, area) in self.areas.iter().enumerate() {
            // 借用は比較の間だけ。書き込みと queue_draw はガードを落としてから
            let changed = {
                let current = self.search_hits[i].borrow();
                geom::rects_changed(&current, &next[i])
            };
            if !changed {
                continue;
            }
            *self.search_hits[i].borrow_mut() = std::mem::take(&mut next[i]);
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

/// ページ座標の矩形 (2 点、順不同) から、選択された引用文と正規化済みの選択矩形群を得る。
/// `connect_drag_update` (下書きの逐次計算) と `connect_drag_end` (確定時) の両方から呼ぶ、
/// 領域計算の唯一の実装。文字情報が無い・空選択・矩形が空のいずれかなら None
fn compute_selection(
    page: &poppler::Page,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> Option<(String, Vec<[f64; 4]>)> {
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
        return None; // 文字情報が無い or 空選択
    }
    let region = page.selected_region(1.0, poppler::SelectionStyle::Glyph, &mut rect)?;
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
        return None;
    }
    Some((quote, rects))
}

/// ページ内の `query` の一致をすべて探し、正規化矩形 (左上原点) にして返す。
/// 一致が無ければ空。全文検索の走査から 1 ページにつき 1 回呼ぶ
///
/// **`find_text` は PDF 座標 (左下原点) で返す。`selected_region` (左上原点) とは
/// 別の系なので上下を反転してから正規化する。** 実測 (下のテスト): 300x200 のページで
/// 下から 150pt の位置 (ベースライン) に置いた文字が y1=145.0 / y2=167.2 で返る。
/// 反転せずに `normalize_rect` へ渡すと、強調がページの上下逆の位置に出る
///
/// 大文字小文字は `find_text` の既定どおり区別しない
pub fn find_hits(page: &poppler::Page, query: &str) -> Vec<[f64; 4]> {
    if query.is_empty() {
        return Vec::new();
    }
    let (page_w, page_h) = page.size();
    page.find_text(query)
        .iter()
        .map(|r| {
            store::normalize_rect(r.x1(), page_h - r.y2(), r.x2(), page_h - r.y1(), page_w, page_h)
        })
        .collect()
}

/// ドラッグ中の下書き選択を消して、消えたときだけ再描画する
fn clear_live_selection(live: &Rc<RefCell<Option<Vec<[f64; 4]>>>>, area: &gtk::DrawingArea) {
    let had = live.borrow_mut().take().is_some();
    if had {
        area.queue_draw();
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
    // live_selection を消す handler はここには付けない。クリックとドラッグは排他
    // (押した点からのしきい値で振り分け) なので、ここに来る時点で live_selection は
    // 既に空か、直前の色ポップオーバー自身の connect_closed で既に消されている。
    // 順序に依存するので、new_popover の呼び出し順を変えるときは要注意
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
    live_selection: Rc<RefCell<Option<Vec<[f64; 4]>>>>,
) {
    let popover = new_popover(anchor, slot, x, y);

    // このポップオーバーが提示する選択そのものを live_selection に書き直す (new_popover が
    // 返った後に書く)。new_popover は前のポップオーバーを同期的に popdown() し得て、それが
    // 同じページの色ポップオーバーだった場合、その connect_closed が同じ live_selection を
    // 消す。実際には autohide のグラブで同一ページ上の多重ドラッグ開始には到達しない見込み
    // だが、防御として new_popover の後に書き、drag_update の最後の値との食い違いも正す
    *live_selection.borrow_mut() = Some(rects.clone());
    anchor.queue_draw();

    // ポップオーバーが閉じたら選択青の下書きを消す。色を選んだ場合はマーカーとして
    // add_highlight 済みなので保存済みハイライトの描画に切り替わるだけで見た目は変わらず、
    // Esc で消した場合は下書きごと消える (どちらの経路でも closed は必ず飛ぶ)
    {
        let live_selection = live_selection.clone();
        // 強参照だと popover → クロージャ → anchor (area) → ... の循環になりうるので弱参照
        let anchor_weak = anchor.downgrade();
        popover.connect_closed(move |_| {
            if let Some(a) = anchor_weak.upgrade() {
                clear_live_selection(&live_selection, &a);
            }
        });
    }

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
                let (r, g, b) = crate::reader::config::color_rgb(&c);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 実テキストを含む最小限の 1 ページ PDF を組み立てて poppler で開く。
    /// xref のオフセットは実際に書いたバイト数から計算するので手打ちの数値に頼らない。
    /// `store::tests::fixture` (ヘッダだけの PDF もどき) では `selected_text` /
    /// `selected_region` を検証できないので、ここだけ本物の内容を持たせる
    fn text_fixture(dir: &std::path::Path) -> poppler::Document {
        let stream = "BT /F1 24 Tf 10 150 Td (Hello selection test line) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] \
              /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                .to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        ];

        let mut buf = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(buf.len());
            buf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref_offset = buf.len();
        buf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        buf.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        buf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
                objects.len() + 1
            )
            .as_bytes(),
        );

        let path = dir.join("text-fixture.pdf");
        std::fs::File::create(&path).expect("作成できること").write_all(&buf).expect("書けること");
        poppler::Document::from_file(&format!("file://{}", path.display()), None)
            .expect("最小 PDF を開けること")
    }

    /// 抑制 (queue_draw を選択が実際に変わったときだけ呼ぶ) の前提になっている性質。
    /// 同じ矩形で `compute_selection` を 2 回呼んでも rects が変わらないこと
    #[test]
    fn compute_selection_is_stable_for_the_same_rect() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = text_fixture(dir.path());
        let page = doc.page(0).expect("1 ページ目");

        let a = compute_selection(&page, 0.0, 0.0, 150.0, 80.0).expect("選択できること");
        let b = compute_selection(&page, 0.0, 0.0, 150.0, 80.0).expect("選択できること");
        assert!(!geom::rects_changed(&a.1, &b.1), "同じ矩形なら rects は変化しない");
    }

    /// 選択範囲が実際に変われば rects も変わり、抑制が効かず再描画されること
    #[test]
    fn compute_selection_differs_when_the_rect_widens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = text_fixture(dir.path());
        let page = doc.page(0).expect("1 ページ目");

        let narrow = compute_selection(&page, 0.0, 0.0, 60.0, 80.0).expect("選択できること");
        let wide = compute_selection(&page, 0.0, 0.0, 300.0, 80.0).expect("選択できること");
        assert!(geom::rects_changed(&narrow.1, &wide.1), "選択範囲が広がれば rects も変わる");
    }

    /// `find_text` の座標系 (左下原点) を左上原点に直せていること。
    /// fixture は 300x200 のページの下から 150pt (= 上から 50pt) の位置に 1 行だけ持つので、
    /// 正しく反転できていれば矩形はページの上半分に来る。反転を忘れると下半分に出る
    #[test]
    fn find_hits_are_flipped_to_top_left_origin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = text_fixture(dir.path());
        let page = doc.page(0).expect("1 ページ目");

        let hits = find_hits(&page, "selection");

        assert_eq!(hits.len(), 1, "1 か所だけ一致すること");
        let [x0, _y0, x1, y1] = hits[0];
        // ここが本題: 反転を忘れると y は下半分 (0.7 台) に出る。
        // 以前ここには `assert!(y0 < y1, ...)` もあったが、`store::normalize_rect` が
        // min/max で並べ替える以上、返る矩形はつねに `y0 <= y1` (等号は高さ 0 の矩形の
        // ときだけ) になり、反転の有無とは無関係に成り立つので削除した
        assert!(y1 < 0.5, "上半分に来ること (左下原点のままなら y は 0.7 台になる): {y1}");
        assert!(x0 > 0.0 && x1 <= 1.0 && x0 < x1, "x は左から右へ: {x0}..{x1}");
    }

    #[test]
    fn find_hits_is_case_insensitive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = text_fixture(dir.path());
        let page = doc.page(0).expect("1 ページ目");

        // find_text の既定は大文字小文字を区別しない。検索欄の入力をそのまま渡す前提
        assert_eq!(find_hits(&page, "SELECTION"), find_hits(&page, "selection"));
    }

    #[test]
    fn find_hits_of_an_absent_word_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = text_fixture(dir.path());
        let page = doc.page(0).expect("1 ページ目");

        assert!(find_hits(&page, "そんな語は無い").is_empty());
    }

    /// 空の検索語で `find_text` を呼ぶと全ページが一致扱いになりかねないので手前で止める
    #[test]
    fn find_hits_with_an_empty_query_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc = text_fixture(dir.path());
        let page = doc.page(0).expect("1 ページ目");

        assert!(find_hits(&page, "").is_empty());
    }
}
