mod control;
mod inkdrop;
mod llm;
mod phrases;
mod scheduler;
mod system;
mod ui;

use gtk4 as gtk;
use gtk::{gio, glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

const QUIT_DELAY_SECS: u64 = 2;
const INBOX_CHECK_FIRST_DELAY_SECS: u64 = 30;
const INBOX_CHECK_INTERVAL_SECS: u64 = 6 * 3600;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.youknow.miryam")
        .build();
    app.connect_activate(|app| {
        if let Err(err) = activate(app) {
            eprintln!("miryam: 起動に失敗しました: {err:#}");
            std::process::exit(1);
        }
    });
    app.run()
}

#[derive(Default)]
struct Timers {
    next_speech: Option<glib::SourceId>,
    hide_bubble: Option<glib::SourceId>,
    llm_request: Option<llm::LlmRequest>,
}

fn activate(app: &gtk::Application) -> anyhow::Result<()> {
    if app.active_window().is_some() {
        return Ok(());
    }

    let started_at = Instant::now();
    let book = Rc::new(phrases::PhraseBook::load()?);
    let ui = Rc::new(ui::build(app, book.skin())?);
    let timers = Rc::new(RefCell::new(Timers::default()));
    let muted = Rc::new(Cell::new(false));
    let quitting = Rc::new(Cell::new(false));
    let book_id_cache: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let inbox_last_notified: Rc<RefCell<Option<chrono::NaiveDate>>> = Rc::new(RefCell::new(None));

    register_actions(
        app, &book, &ui, &timers, &muted, &quitting, &book_id_cache, started_at,
    );

    schedule_next_speech(
        book.clone(), ui.clone(), timers.clone(),
        muted.clone(), quitting.clone(), started_at,
    );
    schedule_chime(
        book.clone(), ui.clone(), timers.clone(),
        muted.clone(), quitting.clone(), started_at,
    );

    schedule_inbox_check(
        book.clone(), ui.clone(), timers.clone(),
        muted.clone(), quitting.clone(),
        book_id_cache.clone(), inbox_last_notified.clone(),
        INBOX_CHECK_FIRST_DELAY_SECS,
    );

    // 起動挨拶 (1 秒後)
    {
        let (book, ui, timers, quitting) =
            (book.clone(), ui.clone(), timers.clone(), quitting.clone());
        glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if !quitting.get() {
                speak_event(&book, &ui, &timers, phrases::EventKind::Boot, started_at);
            }
        });
    }

    let (book_c, ui_c, timers_c, muted_c, quitting_c) = (
        book.clone(), ui.clone(), timers.clone(), muted.clone(), quitting.clone(),
    );
    ui.connect_character_clicked(move || {
        if quitting_c.get() {
            return;
        }
        speak(&book_c, &ui_c, &timers_c, started_at);
        schedule_next_speech(
            book_c.clone(), ui_c.clone(), timers_c.clone(),
            muted_c.clone(), quitting_c.clone(), started_at,
        );
    });

    Ok(())
}

/// テキストを吹き出しに表示し、6 秒後の自動消去タイマーを張り直す
fn show_text(ui: &Rc<ui::MascotUi>, timers: &Rc<RefCell<Timers>>, text: &str) {
    ui.show_bubble(text);
    if let Some(id) = timers.borrow_mut().hide_bubble.take() {
        id.remove();
    }
    let ui_c = ui.clone();
    let timers_c = timers.clone();
    let id = glib::timeout_add_local_once(
        Duration::from_secs(scheduler::BUBBLE_VISIBLE_SECS),
        move || {
            timers_c.borrow_mut().hide_bubble = None;
            ui_c.hide_bubble();
        },
    );
    timers.borrow_mut().hide_bubble = Some(id);
}

/// 辞書から台詞を選んで表示する。進行中の LLM リクエストはキャンセルする (キャンセル規則)
fn speak(
    book: &Rc<phrases::PhraseBook>,
    ui: &Rc<ui::MascotUi>,
    timers: &Rc<RefCell<Timers>>,
    started_at: Instant,
) {
    let pending = timers.borrow_mut().llm_request.take();
    if let Some(req) = pending {
        req.cancel();
    }
    let now = phrases::Snapshot::current(started_at);
    let text = phrases::substitute_placeholders(book.pick(&now), &now);
    show_text(ui, timers, &text);
}

/// 外部由来テキストの発話 (postprocess 済み前提): LLM キャンセル + 表示 + 次回定期発話の張り直し
/// ミュート中でも表示する (自動発話でなく明示的な外部要求のため — mute チェックを足さないこと)
fn external_speak(
    book: &Rc<phrases::PhraseBook>,
    ui: &Rc<ui::MascotUi>,
    timers: &Rc<RefCell<Timers>>,
    muted: &Rc<Cell<bool>>,
    quitting: &Rc<Cell<bool>>,
    started_at: Instant,
    text: &str,
) {
    let pending = timers.borrow_mut().llm_request.take();
    if let Some(req) = pending {
        req.cancel();
    }
    show_text(ui, timers, text);
    schedule_next_speech(
        book.clone(),
        ui.clone(),
        timers.clone(),
        muted.clone(),
        quitting.clone(),
        started_at,
    );
}

/// book 名を ID に解決して cont を呼ぶ (キャッシュあり)。解決不能時は cont(None)
fn resolve_book_id(
    book: Rc<phrases::PhraseBook>,
    quitting: Rc<Cell<bool>>,
    cache: Rc<RefCell<Option<String>>>,
    cont: impl FnOnce(Option<String>) + 'static,
) {
    let cached = cache.borrow().clone();
    if let Some(id) = cached {
        cont(Some(id));
        return;
    }
    let Some(cfg) = book.inkdrop() else {
        cont(None);
        return;
    };
    let name = cfg.book.clone();
    inkdrop::request(cfg, "GET", "/books", None, move |res| {
        if quitting.get() {
            return;
        }
        match res {
            Ok(json) => match inkdrop::find_book_id(&json, &name) {
                Some((id, dup)) => {
                    if dup {
                        eprintln!(
                            "miryam: 同名ノートブック \"{name}\" が複数あります。最初の一致を使います"
                        );
                    }
                    *cache.borrow_mut() = Some(id.clone());
                    cont(Some(id));
                }
                None => {
                    eprintln!("miryam: ノートブック \"{name}\" が見つかりません");
                    cont(None);
                }
            },
            Err(err) => {
                eprintln!("miryam: Inkdrop /books の取得に失敗しました: {}", err.detail);
                cont(None);
            }
        }
    });
}

/// memo: book 解決 → POST /notes → 確認発話 (エラーは毎回 stderr)
fn capture_to_inkdrop(
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    muted: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    cache: Rc<RefCell<Option<String>>>,
    started_at: Instant,
    text: String,
) {
    let (book_r, ui_r, timers_r, muted_r, quitting_r) = (
        book.clone(), ui.clone(), timers.clone(), muted.clone(), quitting.clone(),
    );
    resolve_book_id(book.clone(), quitting.clone(), cache, move |book_id| {
        if quitting_r.get() {
            return;
        }
        let Some(book_id) = book_id else {
            let name = book_r.inkdrop().map(|c| c.book.clone()).unwrap_or_default();
            external_speak(
                &book_r, &ui_r, &timers_r, &muted_r, &quitting_r, started_at,
                &format!("ノートブック \"{name}\" が見つかりません"),
            );
            return;
        };
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let (title, body_text) = inkdrop::capture_note(&text, &date);
        let payload = inkdrop::note_payload(&book_id, &title, &body_text);
        let cfg = book_r.inkdrop().expect("memo は inkdrop 有効時のみ");
        let (book_c, ui_c, timers_c, muted_c, quitting_c) = (
            book_r.clone(), ui_r.clone(), timers_r.clone(), muted_r.clone(), quitting_r.clone(),
        );
        inkdrop::request(cfg, "POST", "/notes", Some(payload), move |res| {
            if quitting_c.get() {
                return;
            }
            match res {
                Ok(_) => external_speak(
                    &book_c, &ui_c, &timers_c, &muted_c, &quitting_c, started_at,
                    "メモを預かりました",
                ),
                Err(err) => {
                    eprintln!(
                        "miryam: Inkdrop への保存に失敗しました (curl exit {:?}): {}",
                        err.curl_exit, err.detail
                    );
                    external_speak(
                        &book_c, &ui_c, &timers_c, &muted_c, &quitting_c, started_at,
                        "Inkdrop に届きませんでした",
                    );
                }
            }
        });
    });
}

/// 自動発話 (見守り用): LLM キャンセル + 表示のみ。定期発話は張り直さない
fn automatic_speak(ui: &Rc<ui::MascotUi>, timers: &Rc<RefCell<Timers>>, text: &str) {
    let pending = timers.borrow_mut().llm_request.take();
    if let Some(req) = pending {
        req.cancel();
    }
    show_text(ui, timers, text);
}

fn warn_inbox_once(detail: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("miryam: Inbox 見守りに失敗しました (以後この警告は抑制): {detail}");
    }
}

/// Inbox 見守り: 固定間隔で件数を確認し、しきい値超過を 1 日 1 回だけ知らせる
fn schedule_inbox_check(
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    muted: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    cache: Rc<RefCell<Option<String>>>,
    last_notified: Rc<RefCell<Option<chrono::NaiveDate>>>,
    delay_secs: u64,
) {
    glib::timeout_add_local_once(Duration::from_secs(delay_secs), move || {
        let enabled = book
            .inkdrop()
            .is_some_and(|c| c.inbox_threshold > 0);
        if enabled && !quitting.get() && !muted.get() {
            run_inbox_check(
                book.clone(), ui.clone(), timers.clone(),
                muted.clone(), quitting.clone(),
                cache.clone(), last_notified.clone(),
            );
        }
        schedule_inbox_check(
            book.clone(), ui.clone(), timers.clone(),
            muted.clone(), quitting.clone(),
            cache.clone(), last_notified.clone(),
            INBOX_CHECK_INTERVAL_SECS,
        );
    });
}

fn run_inbox_check(
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    muted: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    cache: Rc<RefCell<Option<String>>>,
    last_notified: Rc<RefCell<Option<chrono::NaiveDate>>>,
) {
    let (book_r, ui_r, timers_r, muted_r, quitting_r) = (
        book.clone(), ui.clone(), timers.clone(), muted.clone(), quitting.clone(),
    );
    resolve_book_id(book.clone(), quitting.clone(), cache, move |book_id| {
        let Some(book_id) = book_id else {
            warn_inbox_once("ノートブック解決に失敗");
            return;
        };
        let cfg = book_r.inkdrop().expect("見守りは inkdrop 有効時のみ");
        let path = format!(
            "/notes?keyword=bookId:{}&limit={}",
            inkdrop::strip_book_prefix(&book_id),
            inkdrop::NOTES_QUERY_LIMIT
        );
        let threshold = cfg.inbox_threshold;
        inkdrop::request(cfg, "GET", &path, None, move |res| {
            if quitting_r.get() || muted_r.get() {
                return; // 発行後にミュート/終了された場合は発話もマーカーもなし
            }
            let count = match res {
                Ok(json) => match inkdrop::count_notes(&json) {
                    Some(n) => n,
                    None => {
                        warn_inbox_once("応答の解析に失敗");
                        return;
                    }
                },
                Err(err) => {
                    warn_inbox_once(&err.detail);
                    return;
                }
            };
            let today = chrono::Local::now().date_naive();
            if inkdrop::should_notify(count, threshold, *last_notified.borrow(), today) {
                let n = inkdrop::format_count(count, inkdrop::NOTES_QUERY_LIMIT);
                automatic_speak(
                    &ui_r, &timers_r,
                    &format!("Inbox に {n} 件たまっています。そろそろ整理しませんか"),
                );
                *last_notified.borrow_mut() = Some(today);
            }
        });
    });
}

/// イベント台詞を選んで表示する。プールが空なら何もせず false
fn speak_event(
    book: &Rc<phrases::PhraseBook>,
    ui: &Rc<ui::MascotUi>,
    timers: &Rc<RefCell<Timers>>,
    event: phrases::EventKind,
    started_at: Instant,
) -> bool {
    let now = phrases::Snapshot::current(started_at);
    let Some(text) = book.pick_event(event, &now) else {
        return false;
    };
    let text = phrases::substitute_placeholders(text, &now);
    let pending = timers.borrow_mut().llm_request.take();
    if let Some(req) = pending {
        req.cancel();
    }
    show_text(ui, timers, &text);
    true
}

/// 定期発話: [llm] 有効 かつ 確率に当選 かつ in-flight なし なら LLM、それ以外は辞書
fn scheduled_speak(
    book: &Rc<phrases::PhraseBook>,
    ui: &Rc<ui::MascotUi>,
    timers: &Rc<RefCell<Timers>>,
    started_at: Instant,
) {
    let use_llm = book.llm().is_some_and(|cfg| {
        use rand::RngExt;
        timers.borrow().llm_request.is_none()
            && rand::rng().random_range(0.0..1.0) < cfg.probability
    });
    if !use_llm {
        speak(book, ui, timers, started_at);
        return;
    }
    let cfg = book.llm().expect("use_llm なら Some");
    let prompt = llm::build_prompt(cfg, &phrases::Snapshot::current(started_at));
    let (book_c, ui_c, timers_c) = (book.clone(), ui.clone(), timers.clone());
    let req = llm::request_phrase(cfg, &prompt, move |result| {
        timers_c.borrow_mut().llm_request = None;
        match result {
            Some(text) => show_text(&ui_c, &timers_c, &text),
            None => speak(&book_c, &ui_c, &timers_c, started_at),
        }
    });
    timers.borrow_mut().llm_request = Some(req);
}

/// 30〜90 秒後の次回発話をスケジュールする。既存の予約はキャンセルする
fn schedule_next_speech(
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    muted: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    started_at: Instant,
) {
    if let Some(id) = timers.borrow_mut().next_speech.take() {
        id.remove();
    }
    let timers_c = timers.clone();
    let id = glib::timeout_add_local_once(scheduler::next_speech_interval(), move || {
        timers_c.borrow_mut().next_speech = None;
        if !quitting.get() && !muted.get() {
            scheduled_speak(&book, &ui, &timers_c, started_at);
        }
        schedule_next_speech(
            book.clone(), ui.clone(), timers_c.clone(),
            muted.clone(), quitting.clone(), started_at,
        );
    });
    timers.borrow_mut().next_speech = Some(id);
}

/// 次の毎時 0 分に時報を予約する。発火後は再帰的に再予約 (毎回現在時刻から再計算)
fn schedule_chime(
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    muted: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    started_at: Instant,
) {
    use chrono::Timelike;
    let local = chrono::Local::now();
    let wait = scheduler::duration_until_next_hour(local.minute(), local.second());
    glib::timeout_add_local_once(wait, move || {
        if !quitting.get() && !muted.get() {
            speak_event(&book, &ui, &timers, phrases::EventKind::Chime, started_at);
        }
        schedule_chime(
            book.clone(), ui.clone(), timers.clone(),
            muted.clone(), quitting.clone(), started_at,
        );
    });
}

fn register_actions(
    app: &gtk::Application,
    book: &Rc<phrases::PhraseBook>,
    ui: &Rc<ui::MascotUi>,
    timers: &Rc<RefCell<Timers>>,
    muted: &Rc<Cell<bool>>,
    quitting: &Rc<Cell<bool>>,
    book_id_cache: &Rc<RefCell<Option<String>>>,
    started_at: Instant,
) {
    let speak_now = gio::SimpleAction::new("speak-now", None);
    {
        let (book, ui, timers, muted_r, quitting) = (
            book.clone(), ui.clone(), timers.clone(), muted.clone(), quitting.clone(),
        );
        speak_now.connect_activate(move |_, _| {
            if quitting.get() {
                return;
            }
            speak(&book, &ui, &timers, started_at);
            schedule_next_speech(
                book.clone(), ui.clone(), timers.clone(),
                muted_r.clone(), quitting.clone(), started_at,
            );
        });
    }
    app.add_action(&speak_now);

    let mute = gio::SimpleAction::new_stateful("mute", None, &false.to_variant());
    {
        let muted = muted.clone();
        mute.connect_activate(move |action, _| {
            let next = !muted.get();
            muted.set(next);
            action.set_state(&next.to_variant());
        });
    }
    app.add_action(&mute);

    let quit_request = gio::SimpleAction::new("quit-request", None);
    {
        let app_weak = app.downgrade();
        let (book, ui, timers, quitting) =
            (book.clone(), ui.clone(), timers.clone(), quitting.clone());
        quit_request.connect_activate(move |_, _| {
            if quitting.replace(true) {
                return;
            }
            let spoke = speak_event(&book, &ui, &timers, phrases::EventKind::Quit, started_at);
            let app_weak = app_weak.clone();
            if spoke {
                glib::timeout_add_local_once(Duration::from_secs(QUIT_DELAY_SECS), move || {
                    if let Some(app) = app_weak.upgrade() {
                        app.quit();
                    }
                });
            } else if let Some(app) = app_weak.upgrade() {
                app.quit();
            }
        });
    }
    app.add_action(&quit_request);

    // 外部発話: miryam-ctl say <text>
    let say = gio::SimpleAction::new("say", Some(glib::VariantTy::STRING));
    {
        let (book, ui, timers, muted_r, quitting) = (
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
        );
        say.connect_activate(move |_, param| {
            if quitting.get() {
                return;
            }
            let Some(raw) = param.and_then(|v| v.get::<String>()) else {
                return;
            };
            let Some(text) = llm::postprocess(&raw) else {
                return;
            };
            external_speak(&book, &ui, &timers, &muted_r, &quitting, started_at, &text);
        });
    }
    app.add_action(&say);

    // 外部タイマー: miryam-ctl timer <duration> [message...]
    let timer_action = gio::SimpleAction::new("timer", Some(glib::VariantTy::STRING));
    {
        let (book, ui, timers, muted_r, quitting) = (
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
        );
        let app_weak = app.downgrade();
        timer_action.connect_activate(move |_, param| {
            if quitting.get() {
                return;
            }
            let Some(spec) = param.and_then(|v| v.get::<String>()) else {
                return;
            };
            match control::parse_timer_spec(&spec) {
                Ok((wait, message)) => {
                    let text = llm::postprocess(&message)
                        .unwrap_or_else(|| control::DEFAULT_TIMER_MESSAGE.to_string());
                    let (book, ui, timers, muted_r, quitting) = (
                        book.clone(),
                        ui.clone(),
                        timers.clone(),
                        muted_r.clone(),
                        quitting.clone(),
                    );
                    let app_weak = app_weak.clone();
                    glib::timeout_add_local_once(wait, move || {
                        if quitting.get() {
                            return;
                        }
                        external_speak(
                            &book, &ui, &timers, &muted_r, &quitting, started_at, &text,
                        );
                        if let Some(app) = app_weak.upgrade() {
                            let n = gio::Notification::new("miryam");
                            n.set_body(Some(&text));
                            app.send_notification(None, &n);
                        }
                    });
                }
                Err(_) => {
                    external_speak(
                        &book,
                        &ui,
                        &timers,
                        &muted_r,
                        &quitting,
                        started_at,
                        "タイマーの指定がわかりません",
                    );
                }
            }
        });
    }
    app.add_action(&timer_action);

    // Inkdrop キャプチャ: miryam-ctl memo <text>
    let memo = gio::SimpleAction::new("memo", Some(glib::VariantTy::STRING));
    {
        let (book, ui, timers, muted_r, quitting, cache) = (
            book.clone(), ui.clone(), timers.clone(),
            muted.clone(), quitting.clone(), book_id_cache.clone(),
        );
        memo.connect_activate(move |_, param| {
            if quitting.get() || book.inkdrop().is_none() {
                return;
            }
            let Some(text) = param.and_then(|v| v.get::<String>()) else {
                return;
            };
            if text.trim().is_empty() {
                return;
            }
            capture_to_inkdrop(
                book.clone(), ui.clone(), timers.clone(),
                muted_r.clone(), quitting.clone(), cache.clone(),
                started_at, text,
            );
        });
    }
    app.add_action(&memo);
}

#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::{Mutex, MutexGuard};

    /// glib デフォルトメインコンテキストを使う統合テストの直列化ロック。
    /// acquire() は他スレッド保持時に Err を返すため、これ無しでは並列テストが flaky になる
    pub static MAIN_CONTEXT_LOCK: Mutex<()> = Mutex::new(());

    pub fn lock() -> MutexGuard<'static, ()> {
        // 1 本の panic が以降のテストを poison で巻き込まないようにする
        MAIN_CONTEXT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}
