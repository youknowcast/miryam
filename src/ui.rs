use anyhow::Context;
use gtk::{gdk, gdk_pixbuf, gio, glib, prelude::*};
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const DEFAULT_CHARACTER_PNG: &[u8] = include_bytes!("../assets/character.png");
const WINDOW_MARGIN: i32 = 24;
const CHARACTER_WIDTH: i32 = 600;
const CHARACTER_HEIGHT: i32 = 900;
const CONTENT_WIDTH: i32 = 640;

const CSS: &str = "
window { background: transparent; }
.bubble {
  background: rgba(30, 30, 46, 0.92);
  color: #cdd6f4;
  border-radius: 12px;
  padding: 8px 12px;
  font-size: 16px;
}
.chat-entry {
  background: rgba(30, 30, 46, 0.92);
  color: #cdd6f4;
  border-radius: 8px;
  padding: 6px 10px;
  font-size: 16px;
}
";

pub struct MascotUi {
    bubble: gtk::Label,
    picture: gtk::Picture,
    entry: gtk::Entry,
    window: gtk::ApplicationWindow,
}

pub fn build(app: &gtk::Application, skin: Option<&str>) -> anyhow::Result<MascotUi> {
    anyhow::ensure!(
        gtk4_layer_shell::is_supported(),
        "gtk4-layer-shell がこの環境で利用できません (Wayland + layer-shell 対応コンポジタが必要です)"
    );

    load_css();

    let texture = load_character_texture(skin)?;
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_size_request(CHARACTER_WIDTH, CHARACTER_HEIGHT);
    picture.set_halign(gtk::Align::Center);

    // 吹き出しは opacity で表示制御する (レイアウトを常に確保して
    // キャラ位置と input region を安定させるため、visible は使わない)
    let bubble = gtk::Label::new(None);
    bubble.add_css_class("bubble");
    bubble.set_wrap(true);
    bubble.set_max_width_chars(28);
    bubble.set_halign(gtk::Align::Center);
    bubble.set_opacity(0.0);

    // チャット入力欄: 普段は非表示。表示制御は visible で行う
    // (窓は下端アンカーなので、表示時は上に伸びるだけでキャラ位置は安定する)
    let entry = gtk::Entry::new();
    entry.add_css_class("chat-entry");
    entry.set_placeholder_text(Some("話しかける…"));
    entry.set_visible(false);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_width_request(CONTENT_WIDTH);
    root.append(&bubble);
    root.append(&picture);
    root.append(&entry);

    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(Edge::Right, WINDOW_MARGIN);
    window.set_margin(Edge::Bottom, WINDOW_MARGIN);
    window.set_namespace(Some("miryam"));
    window.set_child(Some(&root));
    window.set_keyboard_mode(KeyboardMode::None);

    setup_input_region(&window, &picture, &entry);
    setup_drag(&window, &picture);

    window.present();

    Ok(MascotUi {
        bubble,
        picture,
        entry,
        window,
    })
}

impl MascotUi {
    pub fn show_bubble(&self, text: &str) {
        self.bubble.set_text(text);
        self.bubble.set_opacity(1.0);
    }

    pub fn hide_bubble(&self) {
        self.bubble.set_opacity(0.0);
    }

    /// ドラッグで動かした位置を既定 (右下、マージン WINDOW_MARGIN) に戻す
    pub fn reset_position(&self) {
        self.window.set_margin(Edge::Right, WINDOW_MARGIN);
        self.window.set_margin(Edge::Bottom, WINDOW_MARGIN);
    }

    pub fn connect_character_clicked(&self, f: impl Fn() + 'static) {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gdk::BUTTON_PRIMARY);
        gesture.connect_released(move |_, _, _, _| f());
        self.picture.add_controller(gesture);
    }

    pub fn open_chat(&self) {
        self.entry.set_visible(true);
        self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        self.entry.grab_focus();
    }

    pub fn close_chat(&self) {
        self.entry.set_visible(false);
        // 復帰漏れは他アプリへのキー入力を妨げる — クローズは必ずここを通ること
        self.window.set_keyboard_mode(KeyboardMode::None);
    }

    pub fn connect_chat_submitted(&self, f: impl Fn(String) + 'static) {
        self.entry.connect_activate(move |e| {
            let text = e.text().to_string();
            e.set_text("");
            f(text);
        });
    }

    pub fn connect_chat_escape(&self, f: impl Fn() + 'static) {
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gdk::Key::Escape {
                f();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.entry.add_controller(key);
    }

    /// 右クリックメニューを配線する。メニューモデルは右クリックのたびに
    /// 作り直す (links.toml のホットリロードのため)
    pub fn connect_menu(
        &self,
        chat_enabled: bool,
        links_submenu: impl Fn() -> gio::Menu + 'static,
    ) {
        let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        popover.set_parent(&self.window);
        popover.set_has_arrow(false);

        let gesture = gtk::GestureClick::new();
        gesture.set_button(gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, x, y| {
            let menu = gio::Menu::new();
            menu.append(Some("今すぐ話す"), Some("app.speak-now"));
            if chat_enabled {
                menu.append(Some("話しかける"), Some("app.chat-toggle"));
            }
            menu.append_submenu(Some("リンク集"), &links_submenu());
            menu.append(Some("自動発話を停止"), Some("app.mute"));
            menu.append(Some("位置をリセット"), Some("app.reset-position"));
            menu.append(Some("終了"), Some("app.quit-request"));
            popover.set_menu_model(Some(&menu));
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
        self.window.add_controller(gesture);
    }
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// スキン名から character.png のパスを構築する
fn skin_character_path(config_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    config_dir
        .join("miryam")
        .join("skins")
        .join(name)
        .join("character.png")
}

/// 解決順序: [skin] 指定 (欠落は起動エラー) → 旧 character.png → 埋め込み仮画像
///
/// いずれの経路も表示上限 (CHARACTER_WIDTH x CHARACTER_HEIGHT) に収まるよう
/// アスペクト比を保って縮小デコードする (窓サイズが画像の元ピクセルサイズに
/// 膨張するのを防ぐため)。
fn load_character_texture(skin: Option<&str>) -> anyhow::Result<gdk::Texture> {
    let config_dir = glib::user_config_dir();
    if let Some(name) = skin {
        let path = skin_character_path(&config_dir, name);
        return texture_from_file_scaled(&path).with_context(|| {
            format!(
                "スキン \"{name}\" の画像を読み込めません: {}",
                path.display()
            )
        });
    }
    let legacy = config_dir.join("miryam").join("character.png");
    if legacy.exists() {
        return texture_from_file_scaled(&legacy)
            .with_context(|| format!("{} の読み込みに失敗しました", legacy.display()));
    }
    texture_from_bytes_scaled(DEFAULT_CHARACTER_PNG)
        .context("埋め込みキャラクター画像のデコードに失敗しました")
}

/// ファイルパスの画像を表示上限に収まるよう縮小デコードしてテクスチャ化する。
/// 上限より小さい画像を拡大することはない (scale_to_fit を参照)。
fn texture_from_file_scaled(path: &std::path::Path) -> anyhow::Result<gdk::Texture> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_file(path)?;
    let pixbuf = scale_to_fit(&pixbuf, CHARACTER_WIDTH, CHARACTER_HEIGHT);
    Ok(gdk::Texture::for_pixbuf(&pixbuf))
}

/// 埋め込みバイト列を表示上限に収まるよう縮小デコードしてテクスチャ化する
fn texture_from_bytes_scaled(bytes: &'static [u8]) -> anyhow::Result<gdk::Texture> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes)?;
    loader.close()?;
    let pixbuf = loader
        .pixbuf()
        .context("デコード結果を取得できませんでした")?;
    let pixbuf = scale_to_fit(&pixbuf, CHARACTER_WIDTH, CHARACTER_HEIGHT);
    Ok(gdk::Texture::for_pixbuf(&pixbuf))
}

/// アスペクト比を保ったまま (max_w, max_h) の枠に収まるよう縮小する。
/// 既に枠内に収まっている場合はそのまま返す (拡大はしない)。
fn scale_to_fit(pixbuf: &gdk_pixbuf::Pixbuf, max_w: i32, max_h: i32) -> gdk_pixbuf::Pixbuf {
    let (w, h) = (pixbuf.width(), pixbuf.height());
    if w <= max_w && h <= max_h {
        return pixbuf.clone();
    }
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    let dest_w = ((w as f64 * scale).round() as i32).max(1);
    let dest_h = ((h as f64 * scale).round() as i32).max(1);
    pixbuf
        .scale_simple(dest_w, dest_h, gdk_pixbuf::InterpType::Bilinear)
        .unwrap_or_else(|| pixbuf.clone())
}

/// クリックを受けるのはキャラ画像の矩形 (と表示中はチャット入力欄) のみ。
/// 透明部分と吹き出しは下のアプリへ素通し
fn setup_input_region(window: &gtk::ApplicationWindow, picture: &gtk::Picture, entry: &gtk::Entry) {
    let window_weak = window.downgrade();
    let picture_weak = picture.downgrade();
    let entry_weak = entry.downgrade();
    // map シグナルはコンポジタ都合の再マップ等で複数回発火しうる。connect_layout の
    // ハンドラを毎回追加登録すると同じ GdkSurface に購読が際限なく積み重なるため、
    // 初回のみ登録するようフラグで防ぐ。
    let layout_handler_registered = std::cell::Cell::new(false);
    window.connect_map(move |w| {
        if layout_handler_registered.get() {
            return;
        }
        layout_handler_registered.set(true);

        let Some(surface) = w.surface() else { return };
        let window_weak = window_weak.clone();
        let picture_weak = picture_weak.clone();
        let entry_weak = entry_weak.clone();
        surface.connect_layout(move |surface, _width, _height| {
            let (Some(window), Some(picture), Some(entry)) = (
                window_weak.upgrade(),
                picture_weak.upgrade(),
                entry_weak.upgrade(),
            ) else {
                return;
            };
            let Some(bounds) = picture.compute_bounds(&window) else {
                return;
            };
            let rect = gtk::cairo::RectangleInt::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            );
            let region = gtk::cairo::Region::create_rectangle(&rect);
            // チャット中は入力欄もクリック・フォーカス可能にする
            if gtk::prelude::WidgetExt::is_visible(&entry)
                && let Some(b) = entry.compute_bounds(&window)
            {
                let r = gtk::cairo::RectangleInt::new(
                    b.x() as i32,
                    b.y() as i32,
                    b.width() as i32,
                    b.height() as i32,
                );
                let _ = region.union_rectangle(&r);
            }
            surface.set_input_region(Some(&region));
        });
    });
}

/// クリックと区別してドラッグと確定する移動距離 (px)
const DRAG_THRESHOLD_PX: f64 = 8.0;

/// ドラッグ中のマージン計算: 開始時マージン − オフセットを [0, max] にクランプする。
/// (右下アンカーのため、右/下へのドラッグ = 正のオフセット = マージン減)
fn drag_margin(start: i32, offset: f64, max: Option<i32>) -> i32 {
    let v = ((start as f64 - offset).round() as i32).max(0);
    match max {
        Some(m) => v.min(m.max(0)),
        None => v,
    }
}

/// モニタ内に収めるためのマージン上限 (モニタ情報が取れなければ None = 下限クランプのみ)
fn margin_limits(window: &gtk::ApplicationWindow) -> (Option<i32>, Option<i32>) {
    let Some(surface) = window.surface() else {
        return (None, None);
    };
    let Some(display) = gdk::Display::default() else {
        return (None, None);
    };
    let Some(monitor) = display.monitor_at_surface(&surface) else {
        return (None, None);
    };
    let geo = monitor.geometry();
    (
        Some(geo.width() - window.width()),
        Some(geo.height() - window.height()),
    )
}

/// 左ドラッグでキャラを移動する (右下アンカーからのマージンを動的更新)。
/// 閾値を超えたらシーケンスを Claim し、同じ picture 上のクリック発話
/// ジェスチャを自動キャンセルさせる。閾値未満で離せば従来どおりクリック扱い。
/// 位置は保存しない (再起動で既定位置に戻る)。
///
/// オフセットはウィジェット相対座標で報告されるため、窓が動くと座標系も
/// 一緒にずれ、報告値は「ポインタ移動量 − 窓移動量」の残差になる。
/// 開始時マージンからの絶対計算だと半速追従・ジッタになるので、
/// 「現在マージン − 残差」の逐次適用で自己収束させる
fn setup_drag(window: &gtk::ApplicationWindow, picture: &gtk::Picture) {
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let window_weak = window.downgrade();
    let dragging = std::rc::Rc::new(std::cell::Cell::new(false));
    {
        let dragging = dragging.clone();
        drag.connect_drag_begin(move |_, _, _| dragging.set(false));
    }
    drag.connect_drag_update(move |gesture, dx, dy| {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        if !dragging.get() {
            // Claim 前は窓を動かしていないため dx/dy は純粋なポインタ移動量
            if dx * dx + dy * dy < DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX {
                return;
            }
            dragging.set(true);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
        let (max_r, max_b) = margin_limits(&window);
        let r = drag_margin(window.margin(Edge::Right), dx, max_r);
        let b = drag_margin(window.margin(Edge::Bottom), dy, max_b);
        window.set_margin(Edge::Right, r);
        window.set_margin(Edge::Bottom, b);
    });
    picture.add_controller(drag);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_path_layout() {
        let p = skin_character_path(std::path::Path::new("/cfg"), "asha");
        assert_eq!(
            p,
            std::path::PathBuf::from("/cfg/miryam/skins/asha/character.png")
        );
    }

    #[test]
    fn drag_margin_moves_against_offset() {
        // 右へ 10px ドラッグ → 右マージンは 10 減る
        assert_eq!(drag_margin(24, 10.0, None), 14);
        // 左へ 10px ドラッグ → 右マージンは 10 増える
        assert_eq!(drag_margin(24, -10.0, None), 34);
    }

    #[test]
    fn drag_margin_clamps_to_zero() {
        assert_eq!(drag_margin(24, 100.0, None), 0);
    }

    #[test]
    fn drag_margin_clamps_to_max() {
        assert_eq!(drag_margin(24, -100.0, Some(80)), 80);
        // 上限が負 (窓がモニタより大きい等) でも 0 に丸める
        assert_eq!(drag_margin(24, -100.0, Some(-5)), 0);
    }
}
