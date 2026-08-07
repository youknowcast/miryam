mod phrases;
mod scheduler;
mod system;
mod ui;

use gtk4 as gtk;
use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

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
}

fn activate(app: &gtk::Application) -> anyhow::Result<()> {
    if app.active_window().is_some() {
        return Ok(());
    }

    let started_at = Instant::now();
    let book = Rc::new(phrases::PhraseBook::load()?);
    let ui = Rc::new(ui::build(app)?);
    let timers = Rc::new(RefCell::new(Timers::default()));

    schedule_next_speech(book.clone(), ui.clone(), timers.clone(), started_at);

    let (book_c, ui_c, timers_c) = (book.clone(), ui.clone(), timers.clone());
    ui.connect_character_clicked(move || {
        speak(&book_c, &ui_c, &timers_c, started_at);
        schedule_next_speech(book_c.clone(), ui_c.clone(), timers_c.clone(), started_at);
    });

    Ok(())
}

/// 台詞を表示し、6 秒後の自動消去タイマーを張り直す
fn speak(
    book: &Rc<phrases::PhraseBook>,
    ui: &Rc<ui::MascotUi>,
    timers: &Rc<RefCell<Timers>>,
    started_at: Instant,
) {
    ui.show_bubble(book.pick(&phrases::Snapshot::current(started_at)));
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

/// 30〜90 秒後の次回発話をスケジュールする。既存の予約はキャンセルする
fn schedule_next_speech(
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    started_at: Instant,
) {
    if let Some(id) = timers.borrow_mut().next_speech.take() {
        id.remove();
    }
    let timers_c = timers.clone();
    let id = glib::timeout_add_local_once(scheduler::next_speech_interval(), move || {
        timers_c.borrow_mut().next_speech = None;
        speak(&book, &ui, &timers_c, started_at);
        schedule_next_speech(book.clone(), ui.clone(), timers_c.clone(), started_at);
    });
    timers.borrow_mut().next_speech = Some(id);
}
