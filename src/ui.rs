use anyhow::Context;
use gtk4 as gtk;
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

const DEFAULT_CHARACTER_PNG: &[u8] = include_bytes!("../assets/character.png");
const WINDOW_MARGIN: i32 = 24;
const CHARACTER_WIDTH: i32 = 200;
const CHARACTER_HEIGHT: i32 = 300;
const CONTENT_WIDTH: i32 = 260;

const CSS: &str = "
window { background: transparent; }
.bubble {
  background: rgba(30, 30, 46, 0.92);
  color: #cdd6f4;
  border-radius: 12px;
  padding: 8px 12px;
  font-size: 14px;
}
";

pub struct MascotUi {
    bubble: gtk::Label,
    picture: gtk::Picture,
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
    bubble.set_max_width_chars(16);
    bubble.set_halign(gtk::Align::Center);
    bubble.set_opacity(0.0);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_width_request(CONTENT_WIDTH);
    root.append(&bubble);
    root.append(&picture);

    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(Edge::Right, WINDOW_MARGIN);
    window.set_margin(Edge::Bottom, WINDOW_MARGIN);
    window.set_namespace(Some("miryam"));
    window.set_child(Some(&root));

    setup_input_region(&window, &picture);
    setup_quit_menu(app, &window);

    window.present();

    Ok(MascotUi { bubble, picture })
}

impl MascotUi {
    pub fn show_bubble(&self, text: &str) {
        self.bubble.set_text(text);
        self.bubble.set_opacity(1.0);
    }

    pub fn hide_bubble(&self) {
        self.bubble.set_opacity(0.0);
    }

    pub fn connect_character_clicked(&self, f: impl Fn() + 'static) {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gdk::BUTTON_PRIMARY);
        gesture.connect_released(move |_, _, _, _| f());
        self.picture.add_controller(gesture);
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
fn load_character_texture(skin: Option<&str>) -> anyhow::Result<gdk::Texture> {
    let config_dir = glib::user_config_dir();
    if let Some(name) = skin {
        let path = skin_character_path(&config_dir, name);
        return gdk::Texture::from_filename(&path).with_context(|| {
            format!(
                "スキン \"{name}\" の画像を読み込めません: {}",
                path.display()
            )
        });
    }
    let legacy = config_dir.join("miryam").join("character.png");
    if legacy.exists() {
        return gdk::Texture::from_filename(&legacy)
            .with_context(|| format!("{} の読み込みに失敗しました", legacy.display()));
    }
    gdk::Texture::from_bytes(&glib::Bytes::from_static(DEFAULT_CHARACTER_PNG))
        .context("埋め込みキャラクター画像のデコードに失敗しました")
}

/// クリックを受けるのはキャラ画像の矩形のみ。透明部分と吹き出しは下のアプリへ素通し
fn setup_input_region(window: &gtk::ApplicationWindow, picture: &gtk::Picture) {
    let window_weak = window.downgrade();
    let picture_weak = picture.downgrade();
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
        surface.connect_layout(move |surface, _width, _height| {
            let (Some(window), Some(picture)) = (window_weak.upgrade(), picture_weak.upgrade())
            else {
                return;
            };
            let Some(bounds) = picture.compute_bounds(&window) else { return };
            let rect = gtk::cairo::RectangleInt::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            );
            let region = gtk::cairo::Region::create_rectangle(&rect);
            surface.set_input_region(Some(&region));
        });
    });
}

fn setup_quit_menu(app: &gtk::Application, window: &gtk::ApplicationWindow) {
    let action = gio::SimpleAction::new("quit", None);
    let app_weak = app.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(app) = app_weak.upgrade() {
            app.quit();
        }
    });
    app.add_action(&action);

    let menu = gio::Menu::new();
    menu.append(Some("終了"), Some("app.quit"));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(window);
    popover.set_has_arrow(false);

    let gesture = gtk::GestureClick::new();
    gesture.set_button(gdk::BUTTON_SECONDARY);
    let popover_ref = popover.clone();
    gesture.connect_pressed(move |_, _, x, y| {
        popover_ref.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover_ref.popup();
    });
    window.add_controller(gesture);
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
}
