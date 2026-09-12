//! 発表モード。PDF を 1 ページずつ全画面 / ハーフで表示する。
//!
//! `miryam-reader --present <path>` (および `miryam-ctl present <path>` 経由の
//! miryam からの起動) で使う。既存 reader のレンダーワーカー/キャッシュを流用し、
//! 1 ページだけをフィット表示する。
//!
//! 単一モニタ (ミラーリング) 前提。レイヤーシェルで配置を決めるため、
//! コンポジタのウィンドウ配置に依存しない:
//! - 全画面 (`f`): 四辺アンカーで画面全体を覆う。キーボードは排他で受ける
//! - ハーフ (`h`): 左右どちらかに寄せ、もう片方をデモ用に空ける。キーボードは
//!   OnDemand にしてデモ側のアプリへもキーを渡せるようにする
//!
//! `m` でマスコットの表示/非表示を切り替える (miryam の `set-mascot-visible`
//! アクションを gdbus で叩く)。

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use crate::reader::config::{HalfSide, PresentConfig};
use crate::reader::ui::render_cache::PageRenderCache;
use crate::reader::ui::render_worker::{self, RenderJob};

/// レンダー結果を取り込むポーリング間隔 (reader と同じ)
const POLL_MS: u64 = 16;
/// キャッシュに置けるページ数。前後 1 ページの先読みに十分な数
const CACHE_MAX_PAGES: usize = 8;
/// ページの外側 (レターボックス) の背景色
const BG: (f64, f64, f64) = (0.07, 0.07, 0.08);

/// レイアウト。全画面か、画面半分か
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Full,
    Half,
}

pub fn run(path: PathBuf) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.youknow.miryam.present")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let cfg = crate::reader::config::load_present();
    app.connect_activate(move |app| {
        if let Err(e) = build(app, &path, cfg.clone()) {
            eprintln!("miryam-reader: {e:#}");
            show_fatal(app, &format!("{e:#}"));
        }
    });
    app.run_with_args::<&str>(&[])
}

fn build(app: &gtk::Application, path: &PathBuf, cfg: PresentConfig) -> anyhow::Result<()> {
    let uri = glib::filename_to_uri(path, None)
        .map_err(|e| anyhow::anyhow!("パスを URI にできません: {e}"))?;
    let doc = poppler::Document::from_file(&uri, None)
        .map_err(|e| anyhow::anyhow!("PDF が開けません: {e}"))?;
    if doc.n_pages() == 0 {
        anyhow::bail!("ページがありません");
    }
    let total = doc.n_pages() as usize;
    let mut sizes = Vec::with_capacity(total);
    for i in 0..doc.n_pages() {
        let page = doc
            .page(i)
            .ok_or_else(|| anyhow::anyhow!("{} ページ目が読めません", i + 1))?;
        sizes.push(page.size());
    }
    let sizes = Rc::new(sizes);
    drop(doc); // 描画はワーカーが別途 PDF を開く

    load_css();
    let monitor = primary_monitor_geometry();

    let page_label = gtk::Label::new(None);
    page_label.add_css_class("present-page-label");
    page_label.set_halign(gtk::Align::End);
    page_label.set_valign(gtk::Align::End);
    page_label.set_margin_end(16);
    page_label.set_margin_bottom(12);
    page_label.set_can_target(false);
    set_page_label(&page_label, 1, total);

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&page_label);

    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "miryam-present".into());
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .child(&overlay)
        .build();
    window.init_layer_shell();
    // Top に置く。マスコットも layer-shell で、発表中に表示するときは miryam 側が
    // Overlay へ上げるため、こちらは Top のままにしておく (Overlay では上に出せない)
    window.set_layer(Layer::Top);
    window.set_namespace(Some("miryam-present"));

    let layout: Rc<Cell<Layout>> = Rc::new(Cell::new(Layout::Full));
    let half_side = cfg.half_side;

    // レンダーの共有状態
    let current: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let zoom: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
    let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let cache = Rc::new(RefCell::new(PageRenderCache::new(CACHE_MAX_PAGES)));
    let pending: Rc<RefCell<HashSet<(usize, u64)>>> = Rc::new(RefCell::new(HashSet::new()));
    let (job_tx, done_rx) = render_worker::start(path);

    // 描画。ウィンドウサイズが変わったらフィット倍率を計算し直して再レンダーを依頼する
    {
        let current = current.clone();
        let zoom = zoom.clone();
        let generation = generation.clone();
        let cache = cache.clone();
        let pending = pending.clone();
        let sizes = sizes.clone();
        let job_tx = job_tx.clone();
        area.set_draw_func(move |area, cr, w, h| {
            cr.set_source_rgb(BG.0, BG.1, BG.2);
            let _ = cr.paint();

            let sf = area.scale_factor() as f64;
            let idx = current.get().min(sizes.len().saturating_sub(1));
            let page_size = sizes[idx];
            let want = fit_zoom(page_size, w, h, sf);
            if want > 0.0 && (want - zoom.get()).abs() > f64::EPSILON {
                zoom.set(want);
                // 古いズームの在庫を捨て、周辺ページを新倍率で依頼し直す
                generation.set(generation.get().wrapping_add(1));
                pending.borrow_mut().clear();
                let g = generation.get();
                for i in neighbors(idx, sizes.len()) {
                    request_page(&sizes, &cache, &pending, &job_tx, g, i, want);
                }
            }

            let z = zoom.get();
            if z <= 0.0 {
                return;
            }
            let cached = {
                let mut c = cache.borrow_mut();
                match c.get(idx, z) {
                    Some(s) => Some((s.clone(), 1.0)),
                    // まだ新倍率が無ければ旧倍率を引き伸ばして仮表示する
                    None => c
                        .get_any(idx)
                        .map(|s| (s.clone(), z / (s.width() as f64 / page_size.0))),
                }
            };
            let Some((surface, scale)) = cached else {
                return;
            };
            let dw = surface.width() as f64 * scale / sf;
            let dh = surface.height() as f64 * scale / sf;
            let x = (w as f64 - dw) / 2.0;
            let y = (h as f64 - dh) / 2.0;
            let _ = cr.save();
            cr.translate(x, y);
            cr.scale(scale / sf, scale / sf);
            cr.set_source_surface(&surface, 0.0, 0.0)
                .expect("キャッシュ済みページをソースにできること");
            let _ = cr.paint();
            let _ = cr.restore();
        });
    }

    // ワーカーの結果をメインループへ取り込む
    {
        let render_cache = cache.clone();
        let worker_gen = generation.clone();
        let pending = pending.clone();
        let current = current.clone();
        let area = area.downgrade();
        glib::timeout_add_local(Duration::from_millis(POLL_MS), move || {
            loop {
                match done_rx.try_recv() {
                    Ok(done) => {
                        pending
                            .borrow_mut()
                            .remove(&(done.page, done.zoom.to_bits()));
                        if done.generation != worker_gen.get() {
                            continue;
                        }
                        render_cache.borrow_mut().insert(
                            done.page,
                            done.zoom,
                            done.data.into_inner(),
                        );
                        if done.page == current.get()
                            && let Some(area) = area.upgrade()
                        {
                            area.queue_draw();
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                }
            }
        });
    }

    // ナビゲーション。全画面/ハーフの切替とマスコット表示の切替を配線する
    let go = {
        let current = current.clone();
        let page_label = page_label.clone();
        let area = area.clone();
        let sizes = sizes.clone();
        let zoom = zoom.clone();
        let generation = generation.clone();
        let cache = cache.clone();
        let pending = pending.clone();
        let job_tx = job_tx.clone();
        Rc::new(move |target: isize| {
            let last = total.saturating_sub(1) as isize;
            navigate(
                target.clamp(0, last) as usize,
                total,
                &current,
                &page_label,
                &area,
                &sizes,
                &zoom,
                &generation,
                &cache,
                &pending,
                &job_tx,
            );
        })
    };
    let step = {
        let go = go.clone();
        let current = current.clone();
        Rc::new(move |delta: isize| go(current.get() as isize + delta))
    };

    let mascot_visible: Rc<Cell<bool>> = Rc::new(Cell::new(true));
    let toggle_mascot = {
        let visible = mascot_visible.clone();
        Rc::new(move || {
            let next = !visible.get();
            visible.set(next);
            send_mascot_visible(next);
        })
    };

    // ハーフ表示ではキーボードを OnDemand にするためキーが届かないことがある。
    // マウスだけで全画面へ戻したりマスコットを切り替えられるよう操作バーを出す
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("present-controls");
    controls.set_halign(gtk::Align::End);
    controls.set_valign(gtk::Align::Start);
    controls.set_margin_top(10);
    controls.set_margin_end(10);
    let full_button = gtk::Button::with_label("全画面");
    let mascot_button = gtk::Button::with_label("myriam");
    let close_button = gtk::Button::with_label("終了");

    // 操作バーは数秒で隠す。マウスを動かすと再表示する (投影中の邪魔を減らす)
    let controls_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let reveal_controls: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let timer = controls_timer.clone();
        Rc::new(move || {
            controls.set_visible(true);
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            let controls = controls.clone();
            let timer_for_hide = timer.clone();
            let id = glib::timeout_add_local_once(Duration::from_secs(4), move || {
                timer_for_hide.borrow_mut().take();
                controls.set_visible(false);
            });
            timer.borrow_mut().replace(id);
        })
    };

    let set_full = {
        let window = window.clone();
        let overlay = overlay.clone();
        let layout = layout.clone();
        let reveal = reveal_controls.clone();
        Rc::new(move || {
            if layout.get() == Layout::Full {
                return;
            }
            layout.set(Layout::Full);
            apply_layout(&window, &overlay, Layout::Full, half_side, &monitor);
            reveal();
        })
    };
    let set_half = {
        let window = window.clone();
        let overlay = overlay.clone();
        let layout = layout.clone();
        let reveal = reveal_controls.clone();
        Rc::new(move || {
            if layout.get() == Layout::Half {
                return;
            }
            layout.set(Layout::Half);
            apply_layout(&window, &overlay, Layout::Half, half_side, &monitor);
            reveal();
        })
    };

    {
        let set_full = set_full.clone();
        full_button.connect_clicked(move |_| set_full());
    }
    {
        let toggle_mascot = toggle_mascot.clone();
        mascot_button.connect_clicked(move |_| toggle_mascot());
    }
    {
        let window = window.clone();
        close_button.connect_clicked(move |_| window.close());
    }
    controls.append(&full_button);
    controls.append(&mascot_button);
    controls.append(&close_button);
    overlay.add_overlay(&controls);

    // マウスを動かしたら操作バーを再表示する
    {
        let reveal_motion = reveal_controls.clone();
        let reveal_enter = reveal_controls.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, _, _| reveal_motion());
        motion.connect_enter(move |_, _, _| reveal_enter());
        area.add_controller(motion);
    }

    {
        let window_for_key = window.clone();
        let step = step.clone();
        let go = go.clone();
        let set_full = set_full.clone();
        let set_half = set_half.clone();
        let toggle_mascot = toggle_mascot.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            use gdk::Key;
            match keyval {
                Key::Right | Key::Down | Key::space | Key::Page_Down => step(1),
                Key::Left | Key::Up | Key::Page_Up => step(-1),
                Key::Home => go(0),
                Key::End => go(total.saturating_sub(1) as isize),
                Key::f | Key::F | Key::F11 => set_full(),
                Key::h | Key::H => set_half(),
                Key::m | Key::M => toggle_mascot(),
                Key::Escape | Key::q | Key::Q => {
                    window_for_key.close();
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }

    {
        let next = {
            let step = step.clone();
            Rc::new(move || step(1))
        };
        let prev = {
            let step = step.clone();
            Rc::new(move || step(-1))
        };
        let primary = gtk::GestureClick::new();
        primary.set_button(gdk::BUTTON_PRIMARY);
        primary.connect_released(move |_, _, _, _| next());
        area.add_controller(primary);

        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        secondary.connect_released(move |_, _, _, _| prev());
        area.add_controller(secondary);
    }

    {
        let step = step.clone();
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(move |_, _dx, dy| {
            if dy > 0.0 {
                step(1);
            } else if dy < 0.0 {
                step(-1);
            }
            glib::Propagation::Stop
        });
        area.add_controller(scroll);
    }

    // 閉じるときにマスコットを戻す (miryam 側の子プロセス終了処理と二重でも無害)
    {
        let visible = mascot_visible.clone();
        window.connect_close_request(move |_| {
            if !visible.get() {
                send_mascot_visible(true);
            }
            glib::Propagation::Proceed
        });
    }

    // 初期配置は全画面。アンカーは present 前に確定させる
    apply_layout(&window, &overlay, Layout::Full, half_side, &monitor);
    window.present();
    // 操作バーを少しの間見せて、`m` などを知らせる (以後はマウスを動かすと再表示)
    reveal_controls();
    // 発表に集中できるよう、開始時はマスコットを隠して自動発話も止める。
    // 出したいときは `m` で戻せる
    if mascot_visible.get() {
        mascot_visible.set(false);
        send_mascot_visible(false);
    }
    Ok(())
}

/// 画面いっぱい (四辺アンカー) か、左右どちらかのハーフかを適用する。
/// 操作バーの表示/非表示は `reveal_controls` 側で制御する
fn apply_layout(
    window: &gtk::ApplicationWindow,
    overlay: &gtk::Overlay,
    layout: Layout,
    half_side: HalfSide,
    monitor: &gdk::Rectangle,
) {
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, false);
    }
    match layout {
        Layout::Full => {
            overlay.set_width_request(-1);
            for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                window.set_anchor(edge, true);
            }
            window.set_keyboard_mode(KeyboardMode::Exclusive);
        }
        Layout::Half => {
            // 片面だけアンカーすると幅は内容の natural size になる。幅を画面半分に固定する
            overlay.set_width_request((monitor.width() / 2).max(1));
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            match half_side {
                HalfSide::Left => window.set_anchor(Edge::Left, true),
                HalfSide::Right => window.set_anchor(Edge::Right, true),
            }
            // ハーフではデモ側のアプリにもキーを渡せるよう、クリックで focus したときだけ受ける
            window.set_keyboard_mode(KeyboardMode::OnDemand);
        }
    }
}

/// フィット倍率 (デバイスピクセル/ポイント)。エリアが未確定なら 0.0
fn fit_zoom(page: (f64, f64), w: i32, h: i32, scale_factor: f64) -> f64 {
    if w <= 0 || h <= 0 || page.0 <= 0.0 || page.1 <= 0.0 {
        return 0.0;
    }
    (w as f64 / page.0).min(h as f64 / page.1) * scale_factor
}

/// `idx` と前後 1 ページ
fn neighbors(idx: usize, total: usize) -> impl Iterator<Item = usize> {
    let lo = idx.saturating_sub(1);
    let hi = (idx + 1).min(total.saturating_sub(1));
    lo..=hi
}

fn request_page(
    sizes: &Rc<Vec<(f64, f64)>>,
    cache: &Rc<RefCell<PageRenderCache>>,
    pending: &Rc<RefCell<HashSet<(usize, u64)>>>,
    job_tx: &mpsc::Sender<RenderJob>,
    generation: u64,
    index: usize,
    zoom: f64,
) {
    if zoom <= 0.0 || index >= sizes.len() {
        return;
    }
    if cache.borrow_mut().get(index, zoom).is_some() {
        return;
    }
    if !pending.borrow_mut().insert((index, zoom.to_bits())) {
        return;
    }
    let _ = job_tx.send(RenderJob {
        page: index,
        zoom,
        generation,
    });
}

#[allow(clippy::too_many_arguments)]
fn navigate(
    target: usize,
    total: usize,
    current: &Rc<Cell<usize>>,
    page_label: &gtk::Label,
    area: &gtk::DrawingArea,
    sizes: &Rc<Vec<(f64, f64)>>,
    zoom: &Rc<Cell<f64>>,
    generation: &Rc<Cell<u64>>,
    cache: &Rc<RefCell<PageRenderCache>>,
    pending: &Rc<RefCell<HashSet<(usize, u64)>>>,
    job_tx: &mpsc::Sender<RenderJob>,
) {
    if target >= total || target == current.get() {
        return;
    }
    current.set(target);
    set_page_label(page_label, target + 1, total);
    let z = zoom.get();
    let g = generation.get();
    for i in neighbors(target, total) {
        request_page(sizes, cache, pending, job_tx, g, i, z);
    }
    area.queue_draw();
}

fn set_page_label(label: &gtk::Label, page: usize, total: usize) {
    label.set_text(&format!("{page} / {total}"));
}

fn primary_monitor_geometry() -> gdk::Rectangle {
    gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_downcast::<gdk::Monitor>()
        .map(|m| m.geometry())
        .unwrap_or_else(|| gdk::Rectangle::new(0, 0, 1920, 1080))
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        ".present-page-label { background-color: rgba(0,0,0,0.45); color: #f0f0f0; \
         padding: 2px 10px; border-radius: 10px; font-size: 13px; } \
         .present-controls { background-color: rgba(0,0,0,0.55); padding: 4px 6px; \
         border-radius: 10px; } \
         .present-controls button { min-height: 22px; padding: 0 8px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// miryam のマスコット表示/自動発話を切り替える。
/// 起動していなければ黙って諦める (reader の `notify_miryam` と同じ流儀)
fn send_mascot_visible(visible: bool) {
    let arg = format!("[<{visible}>]");
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
        "set-mascot-visible".as_ref(),
        arg.as_ref(),
        "{}".as_ref(),
        "--timeout".as_ref(),
        "2".as_ref(),
    ];
    match gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE,
    ) {
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
        .text("PDF を発表モードで開けませんでした")
        .secondary_text(msg)
        .build();
    let app = app.clone();
    dialog.connect_response(move |d, _| {
        d.close();
        app.quit();
    });
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_zoom_uses_the_limiting_dimension() {
        // 100x200 のページを 1000x1000 に収める → 高さが制約 → 5 倍
        assert!((fit_zoom((100.0, 200.0), 1000, 1000, 1.0) - 5.0).abs() < 1e-9);
        // 横向きは幅が制約
        assert!((fit_zoom((400.0, 100.0), 1000, 1000, 1.0) - 2.5).abs() < 1e-9);
        // デバイススケールを掛ける
        assert!((fit_zoom((100.0, 200.0), 1000, 1000, 2.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn fit_zoom_is_zero_until_allocated() {
        assert_eq!(fit_zoom((100.0, 200.0), 0, 1000, 1.0), 0.0);
        assert_eq!(fit_zoom((0.0, 200.0), 1000, 1000, 1.0), 0.0);
    }

    #[test]
    fn neighbors_covers_previous_and_next_within_bounds() {
        assert_eq!(neighbors(0, 5).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(neighbors(2, 5).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(neighbors(4, 5).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(neighbors(0, 1).collect::<Vec<_>>(), vec![0]);
    }
}
