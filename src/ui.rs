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
    /// [skin] 名。None なら表情差分は無効 (legacy 配置・埋め込みデフォルト)
    skin: Option<String>,
    /// 通常表情 (character.png)。set_face(None) で戻す先
    default_texture: gdk::Texture,
    /// 表情テクスチャの遅延ロードキャッシュ。None = ロード失敗済み (再試行しない)
    face_cache: std::cell::RefCell<std::collections::HashMap<String, Option<gdk::Texture>>>,
    /// 透かし背景 (backdrop.png) の遅延ロードキャッシュ。成功時のみ保持し、
    /// 未配置の間は毎回ファイルを確認する (後から置いても再起動不要にするため)
    backdrop_cache: std::cell::RefCell<Option<gdk::Texture>>,
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
        skin: skin.map(str::to_string),
        default_texture: texture,
        face_cache: std::cell::RefCell::new(std::collections::HashMap::new()),
        backdrop_cache: std::cell::RefCell::new(None),
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

    /// 表情を切り替える。None = 通常 (character.png)。
    /// 表情画像は初回使用時に読み込んでキャッシュし、失敗した表情は
    /// 一度だけ警告して以後は通常にフォールバックする ([skin] 未設定も通常のまま)
    pub fn set_face(&self, face: Option<&str>) {
        let texture = face.and_then(|f| self.face_texture(f));
        match texture {
            Some(t) => self.picture.set_paintable(Some(&t)),
            None => self.picture.set_paintable(Some(&self.default_texture)),
        }
    }

    fn face_texture(&self, face: &str) -> Option<gdk::Texture> {
        let skin = self.skin.as_deref()?;
        self.face_cache
            .borrow_mut()
            .entry(face.to_string())
            .or_insert_with(|| {
                let path = skin_face_path(&glib::user_config_dir(), skin, face);
                match texture_from_file_scaled(&path) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!(
                            "miryam: 表情 \"{face}\" を読み込めません。通常表情を使います ({}): {e:#}",
                            path.display()
                        );
                        None
                    }
                }
            })
            .clone()
    }

    /// 会話窓・ニュース窓の透かし背景用テクスチャ。
    /// スキンに backdrop.png (バストアップなどの専用絵) があればそれを使い、
    /// なければ通常立ち絵にフォールバックする
    pub fn backdrop_texture(&self) -> gdk::Texture {
        if let Some(t) = self.backdrop_cache.borrow().as_ref() {
            return t.clone();
        }
        let Some(skin) = self.skin.as_deref() else {
            return self.default_texture.clone();
        };
        let path = skin_backdrop_path(&glib::user_config_dir(), skin);
        if !path.exists() {
            return self.default_texture.clone();
        }
        match texture_from_file_scaled(&path) {
            Ok(t) => {
                *self.backdrop_cache.borrow_mut() = Some(t.clone());
                t
            }
            Err(e) => {
                eprintln!(
                    "miryam: backdrop.png を読み込めません。立ち絵を使います ({}): {e:#}",
                    path.display()
                );
                self.default_texture.clone()
            }
        }
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
    /// 作り直す (links.toml のホットリロードのため)。
    /// chat_modes が空ならチャット項目は出さない
    pub fn connect_menu(
        &self,
        chat_modes: Vec<String>,
        news_enabled: bool,
        library_enabled: bool,
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
            if !chat_modes.is_empty() {
                let sub = gio::Menu::new();
                for name in &chat_modes {
                    let item = gio::MenuItem::new(Some(name), None);
                    item.set_action_and_target_value(
                        Some("app.chat-mode"),
                        Some(&name.to_variant()),
                    );
                    sub.append_item(&item);
                }
                menu.append_submenu(Some("話しかける"), &sub);
            }
            if news_enabled {
                menu.append(Some("ニュースを見る"), Some("app.news-show"));
            }
            if library_enabled {
                menu.append(Some("本棚"), Some("app.library-show"));
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

/// 会話窓・ニュース窓の透かし背景の不透明度
const BACKDROP_OPACITY: f64 = 0.3;

/// コンテンツの下層にキャラ絵を透かしで敷いた Overlay を返す。
/// 絵は右下寄せ (デスクトップのマスコット位置とそろえる)・contain フィット・入力透過。
/// GtkOverlay は child がサイズを決めるため、content 側を measure 対象にして
/// 窓のサイズ要求が Picture の原寸に引きずられないようにする
fn with_backdrop(texture: &gdk::Texture, content: &impl IsA<gtk::Widget>) -> gtk::Overlay {
    let picture = gtk::Picture::for_paintable(texture);
    // contain フィットは Picture の既定 (keep-aspect-ratio=true)。
    // ContentFit API は gtk4 の v4_8 feature が必要なため使わない (依存は v4_6 のまま)
    picture.set_halign(gtk::Align::End);
    picture.set_valign(gtk::Align::End);
    picture.set_opacity(BACKDROP_OPACITY);
    picture.set_can_target(false);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(content);
    overlay.set_measure_overlay(content, true);
    overlay
}

/// ニュースダイジェスト用の通常ウィンドウ (layer shell ではないフロート窓)。
/// slot で同時 1 枚を保証する: 開き直しは前の窓を閉じてから
pub fn show_news_window(
    app: &gtk::Application,
    slot: &std::rc::Rc<std::cell::RefCell<Option<gtk::Window>>>,
    title: &str,
    body: &str,
    backdrop: &gdk::Texture,
) {
    if let Some(prev) = slot.borrow_mut().take() {
        prev.close();
    }
    let label = gtk::Label::new(Some(body));
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_xalign(0.0);
    label.set_valign(gtk::Align::Start);
    label.set_margin_top(12);
    label.set_margin_bottom(12);
    label.set_margin_start(16);
    label.set_margin_end(16);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_child(Some(&label));

    let window = gtk::Window::new();
    window.set_application(Some(app));
    window.set_title(Some(title));
    window.set_default_size(520, 640);
    window.set_child(Some(&with_backdrop(backdrop, &scrolled)));

    let key = gtk::EventControllerKey::new();
    let win_c = window.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gdk::Key::Escape {
            win_c.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);

    // 閉じられたら slot を空に (Esc・タイトルバー双方この経路を通る)
    let slot_c = slot.clone();
    window.connect_close_request(move |_| {
        slot_c.borrow_mut().take();
        glib::Propagation::Proceed
    });

    *slot.borrow_mut() = Some(window.clone());
    window.present();
}

/// 本棚ウィンドウ (layer shell ではないフロート窓)。行を選ぶと on_open が呼ばれる。
/// slot で同時 1 枚を保証する: 開き直しは前の窓を閉じてから (show_news_window と同じ)
pub fn show_library_window(
    app: &gtk::Application,
    slot: &std::rc::Rc<std::cell::RefCell<Option<gtk::Window>>>,
    entries: Vec<crate::reader::library::LibraryEntry>,
    backdrop: &gdk::Texture,
    on_open: impl Fn(&std::path::Path) + 'static,
) {
    if let Some(prev) = slot.borrow_mut().take() {
        prev.close();
    }

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    if entries.is_empty() {
        let empty = gtk::Label::new(Some("PDF がありません"));
        empty.set_margin_top(16);
        empty.set_margin_bottom(16);
        list.append(&empty);
    }
    let on_open = std::rc::Rc::new(on_open);
    for e in entries {
        let button = gtk::Button::with_label(&crate::reader::library::format_entry(&e));
        button.set_has_frame(false);
        let path = e.path.clone();
        let on_open = on_open.clone();
        button.connect_clicked(move |_| on_open(&path));
        list.append(&button);
    }

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&list));

    let window = gtk::Window::new();
    window.set_application(Some(app));
    window.set_title(Some("本棚"));
    window.set_default_size(520, 480);
    window.set_child(Some(&with_backdrop(backdrop, &scrolled)));

    let key = gtk::EventControllerKey::new();
    let window_for_key = window.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gdk::Key::Escape {
            window_for_key.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);

    // 閉じられたら slot を空に (Esc・タイトルバー双方この経路を通る)
    let slot_for_close = slot.clone();
    window.connect_close_request(move |_| {
        slot_for_close.borrow_mut().take();
        glib::Propagation::Proceed
    });

    *slot.borrow_mut() = Some(window.clone());
    window.present();
}

/// miryam-reader は自分と同じディレクトリにあるものを使う。
/// 親ディレクトリが取れなければ PATH 解決に任せて名前だけ返す
pub fn reader_exe_path(current_exe: &std::path::Path) -> std::path::PathBuf {
    match current_exe.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("miryam-reader"),
        _ => std::path::PathBuf::from("miryam-reader"),
    }
}

/// Entry と選択肢ボタン共通の送信コールバックの型 (clippy::type_complexity 回避)
type SubmitCallback = std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<dyn Fn(String)>>>>;

/// 議論系チャットモード用の会話ウィンドウ (ニュース窓と同系のフロート窓)。
/// 同時 1 枚の保証とセッション管理は main.rs 側の責務
pub struct ChatWindow {
    window: gtk::Window,
    history: gtk::Box,
    scrolled: gtk::ScrolledWindow,
    choices: gtk::Box,
    entry: gtk::Entry,
    /// 考え中の「miryam: ……」行。finish_reply で置換して None に戻す
    pending: std::rc::Rc<std::cell::RefCell<Option<gtk::Label>>>,
    /// Entry と選択肢ボタン共通の送信コールバック
    on_submit: SubmitCallback,
}

pub fn build_chat_window(
    app: &gtk::Application,
    title: &str,
    backdrop: &gdk::Texture,
) -> ChatWindow {
    let history = gtk::Box::new(gtk::Orientation::Vertical, 8);
    history.set_margin_top(12);
    history.set_margin_bottom(12);
    history.set_margin_start(16);
    history.set_margin_end(16);
    history.set_valign(gtk::Align::Start);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_child(Some(&history));
    scrolled.set_vexpand(true);

    let choices = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    choices.set_margin_start(16);
    choices.set_margin_end(16);
    choices.set_visible(false);

    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("話しかける…"));
    entry.set_margin_top(8);
    entry.set_margin_bottom(12);
    entry.set_margin_start(16);
    entry.set_margin_end(16);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.append(&scrolled);
    root.append(&choices);
    root.append(&entry);

    let window = gtk::Window::new();
    window.set_application(Some(app));
    window.set_title(Some(title));
    window.set_default_size(520, 640);
    window.set_child(Some(&with_backdrop(backdrop, &root)));

    let key = gtk::EventControllerKey::new();
    let win_c = window.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gdk::Key::Escape {
            win_c.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);

    let on_submit: SubmitCallback = std::rc::Rc::new(std::cell::RefCell::new(None));
    {
        let on_submit = on_submit.clone();
        entry.connect_activate(move |e| {
            let text = e.text().to_string();
            e.set_text("");
            let cb = on_submit.borrow().clone();
            if let Some(cb) = cb {
                cb(text);
            }
        });
    }

    window.present();
    entry.grab_focus();

    ChatWindow {
        window,
        history,
        scrolled,
        choices,
        entry,
        pending: std::rc::Rc::new(std::cell::RefCell::new(None)),
        on_submit,
    }
}

impl ChatWindow {
    fn append_line(&self, text: &str) -> gtk::Label {
        let label = gtk::Label::new(Some(text));
        label.set_wrap(true);
        label.set_selectable(true);
        label.set_xalign(0.0);
        self.history.append(&label);
        self.scroll_to_bottom();
        label
    }

    /// レイアウト確定後に最下部へ (append 直後は upper が古いので idle で)
    fn scroll_to_bottom(&self) {
        let adj = self.scrolled.vadjustment();
        glib::idle_add_local_once(move || adj.set_value(adj.upper() - adj.page_size()));
    }

    pub fn push_user(&self, text: &str) {
        self.append_line(&format!("あなた: {text}"));
    }

    pub fn begin_reply(&self) {
        let label = self.append_line("miryam: ……");
        *self.pending.borrow_mut() = Some(label);
    }

    pub fn finish_reply(&self, text: Option<&str>) {
        let Some(label) = self.pending.borrow_mut().take() else {
            return;
        };
        match text {
            Some(t) => label.set_text(&format!("miryam: {t}")),
            None => label.set_text("(応答が得られませんでした)"),
        }
        self.scroll_to_bottom();
    }

    pub fn set_choices(&self, choices: &[String]) {
        while let Some(child) = self.choices.first_child() {
            self.choices.remove(&child);
        }
        self.choices.set_visible(!choices.is_empty());
        for choice in choices {
            let button = gtk::Button::with_label(choice);
            let on_submit = self.on_submit.clone();
            let text = choice.clone();
            button.connect_clicked(move |_| {
                let cb = on_submit.borrow().clone();
                if let Some(cb) = cb {
                    cb(text.clone());
                }
            });
            self.choices.append(&button);
        }
    }

    pub fn set_busy(&self, busy: bool) {
        self.entry.set_sensitive(!busy);
        self.choices.set_sensitive(!busy);
    }

    pub fn connect_submitted(&self, f: impl Fn(String) + 'static) {
        *self.on_submit.borrow_mut() = Some(std::rc::Rc::new(f));
    }

    pub fn connect_closed(&self, f: impl Fn() + 'static) {
        self.window.connect_close_request(move |_| {
            f();
            glib::Propagation::Proceed
        });
    }

    pub fn close(&self) {
        self.window.close();
    }

    /// close イベントの同一性ガード用に内部ウィンドウへの弱参照を返す
    pub fn downgrade(&self) -> glib::WeakRef<gtk::Window> {
        self.window.downgrade()
    }

    /// この ChatWindow が指定のウィンドウを保持しているか (GObject ポインタ比較)
    pub fn is_window(&self, window: &gtk::Window) -> bool {
        &self.window == window
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

/// スキン名から透かし背景 backdrop.png のパスを構築する
fn skin_backdrop_path(config_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    config_dir
        .join("miryam")
        .join("skins")
        .join(name)
        .join("backdrop.png")
}

/// スキン名と表情名から character-<face>.png のパスを構築する
fn skin_face_path(config_dir: &std::path::Path, name: &str, face: &str) -> std::path::PathBuf {
    config_dir
        .join("miryam")
        .join("skins")
        .join(name)
        .join(format!("character-{face}.png"))
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
    fn skin_face_path_layout() {
        let p = skin_face_path(std::path::Path::new("/cfg"), "asha", "happy");
        assert_eq!(
            p,
            std::path::PathBuf::from("/cfg/miryam/skins/asha/character-happy.png")
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

    #[test]
    fn reader_exe_sits_next_to_the_mascot() {
        assert_eq!(
            reader_exe_path(std::path::Path::new("/opt/miryam/bin/miryam")),
            std::path::PathBuf::from("/opt/miryam/bin/miryam-reader")
        );
    }

    #[test]
    fn reader_exe_falls_back_to_bare_name() {
        assert_eq!(
            reader_exe_path(std::path::Path::new("miryam")),
            std::path::PathBuf::from("miryam-reader")
        );
    }
}
