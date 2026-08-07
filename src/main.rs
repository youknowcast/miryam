mod control;
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

    register_actions(app, &book, &ui, &timers, &muted, &quitting, started_at);

    schedule_next_speech(
        book.clone(), ui.clone(), timers.clone(),
        muted.clone(), quitting.clone(), started_at,
    );
    schedule_chime(
        book.clone(), ui.clone(), timers.clone(),
        muted.clone(), quitting.clone(), started_at,
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
}
