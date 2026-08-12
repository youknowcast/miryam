mod chat;
mod control;
mod inkdrop;
mod links;
mod llm;
mod news;
mod phrases;
mod scheduler;
mod system;
mod ui;

use gtk::{gio, glib, prelude::*};
use gtk4 as gtk;
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

/// タイマー・進行中リクエスト・チャット状態のコンテナ
/// (名前は歴史的経緯。llm_request 以降、実行時状態全般を持つ)
#[derive(Default)]
struct Timers {
    next_speech: Option<glib::SourceId>,
    hide_bubble: Option<glib::SourceId>,
    llm_request: Option<llm::LlmRequest>,
    chat_request: Option<llm::LlmRequest>,
    chat_idle: Option<glib::SourceId>,
    /// Some = チャットセッション中 (開始/終了は open_or_toggle_chat / close_chat_session のみが触る)
    chat_session: Option<chat::ChatSession>,
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
    let chat_book_id_cache: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // [chat] あり + [inkdrop] なし: 会話は可能だが履歴保存はされない (一度だけ知らせる)
    if book.chat().is_some() && book.inkdrop().is_none() {
        eprintln!("miryam: [inkdrop] が未設定のため会話ログは保存されません");
    }

    register_actions(
        app,
        &book,
        &ui,
        &timers,
        &muted,
        &quitting,
        &book_id_cache,
        &chat_book_id_cache,
        started_at,
    );

    schedule_next_speech(
        book.clone(),
        ui.clone(),
        timers.clone(),
        muted.clone(),
        quitting.clone(),
        started_at,
    );
    schedule_chime(
        book.clone(),
        ui.clone(),
        timers.clone(),
        muted.clone(),
        quitting.clone(),
        started_at,
    );

    if book.inkdrop().is_some_and(|c| c.inbox_threshold > 0) {
        schedule_inbox_check(
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
            book_id_cache.clone(),
            inbox_last_notified.clone(),
            INBOX_CHECK_FIRST_DELAY_SECS,
        );
    }

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
        book.clone(),
        ui.clone(),
        timers.clone(),
        muted.clone(),
        quitting.clone(),
    );
    ui.connect_character_clicked(move || {
        if quitting_c.get() || timers_c.borrow().chat_session.is_some() {
            return;
        }
        speak(&book_c, &ui_c, &timers_c, started_at);
        schedule_next_speech(
            book_c.clone(),
            ui_c.clone(),
            timers_c.clone(),
            muted_c.clone(),
            quitting_c.clone(),
            started_at,
        );
    });

    Ok(())
}

/// テキストを吹き出しに表示し、secs 秒後の自動消去タイマーを張り直す
fn show_text_for(ui: &Rc<ui::MascotUi>, timers: &Rc<RefCell<Timers>>, text: &str, secs: u64) {
    // 吹き出しの掛け替え時は表情も通常へ (辞書発話は show の直後に set_face で上書きする)
    ui.set_face(None);
    ui.show_bubble(text);
    if let Some(id) = timers.borrow_mut().hide_bubble.take() {
        id.remove();
    }
    let ui_c = ui.clone();
    let timers_c = timers.clone();
    let id = glib::timeout_add_local_once(Duration::from_secs(secs), move || {
        timers_c.borrow_mut().hide_bubble = None;
        ui_c.hide_bubble();
        ui_c.set_face(None);
    });
    timers.borrow_mut().hide_bubble = Some(id);
}

/// 既定 6 秒表示 (既存呼び出し互換)
fn show_text(ui: &Rc<ui::MascotUi>, timers: &Rc<RefCell<Timers>>, text: &str) {
    show_text_for(ui, timers, text, scheduler::BUBBLE_VISIBLE_SECS);
}

/// 自動消去なしの表示 (チャットの「……」考え中プレースホルダ用)
fn show_text_persistent(ui: &Rc<ui::MascotUi>, timers: &Rc<RefCell<Timers>>, text: &str) {
    if let Some(id) = timers.borrow_mut().hide_bubble.take() {
        id.remove();
    }
    ui.set_face(None);
    ui.show_bubble(text);
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
    let (text, face) = book.pick(&now);
    let text = phrases::substitute_placeholders(text, &now);
    show_text(ui, timers, &text);
    ui.set_face(face);
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

/// resolve_book_id の失敗理由
enum ResolveError {
    /// /books は取れたが名前一致なし
    NotFound,
    /// /books のリクエスト自体が失敗
    Request(inkdrop::RequestError),
}

/// book 名を ID に解決して cont を呼ぶ (キャッシュあり)。
/// 注意: cont は quitting 中でも呼ばれる (終了時のチャット保存を通すため)。
/// 発話やマーカー更新の要否は cont 側で判断すること。
fn resolve_book_id(
    book: Rc<phrases::PhraseBook>,
    cache: Rc<RefCell<Option<String>>>,
    name: String,
    cont: impl FnOnce(Result<String, ResolveError>) + 'static,
) {
    let cached = cache.borrow().clone();
    if let Some(id) = cached {
        cont(Ok(id));
        return;
    }
    let Some(cfg) = book.inkdrop() else {
        cont(Err(ResolveError::NotFound));
        return;
    };
    inkdrop::request(cfg, "GET", "/books", None, move |res| match res {
        Ok(json) => match inkdrop::find_book_id(&json, &name) {
            Some((id, dup)) => {
                if dup {
                    eprintln!(
                        "miryam: 同名ノートブック \"{name}\" が複数あります。最初の一致を使います"
                    );
                }
                *cache.borrow_mut() = Some(id.clone());
                cont(Ok(id));
            }
            None => cont(Err(ResolveError::NotFound)),
        },
        Err(err) => cont(Err(ResolveError::Request(err))),
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
    let cfg_book_name = book
        .inkdrop()
        .expect("memo は inkdrop 有効時のみ")
        .book
        .clone();
    let (book_r, ui_r, timers_r, muted_r, quitting_r) = (
        book.clone(),
        ui.clone(),
        timers.clone(),
        muted.clone(),
        quitting.clone(),
    );
    let name_c = cfg_book_name.clone();
    resolve_book_id(book.clone(), cache, cfg_book_name, move |resolved| {
        if quitting_r.get() {
            return;
        }
        let book_id = match resolved {
            Ok(id) => id,
            Err(ResolveError::NotFound) => {
                eprintln!("miryam: ノートブック \"{name_c}\" が見つかりません");
                external_speak(
                    &book_r,
                    &ui_r,
                    &timers_r,
                    &muted_r,
                    &quitting_r,
                    started_at,
                    &format!("ノートブック \"{name_c}\" が見つかりません"),
                );
                return;
            }
            Err(ResolveError::Request(err)) => {
                eprintln!(
                    "miryam: Inkdrop への接続に失敗しました (curl exit {:?}): {}",
                    err.curl_exit, err.detail
                );
                external_speak(
                    &book_r,
                    &ui_r,
                    &timers_r,
                    &muted_r,
                    &quitting_r,
                    started_at,
                    "Inkdrop に届きませんでした",
                );
                return;
            }
        };
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let (title, body_text) = inkdrop::capture_note(&text, &date);
        let payload = inkdrop::note_payload(&book_id, &title, &body_text);
        let cfg = book_r.inkdrop().expect("memo は inkdrop 有効時のみ");
        let (book_c, ui_c, timers_c, muted_c, quitting_c) = (
            book_r.clone(),
            ui_r.clone(),
            timers_r.clone(),
            muted_r.clone(),
            quitting_r.clone(),
        );
        inkdrop::request(cfg, "POST", "/notes", Some(payload), move |res| {
            if quitting_c.get() {
                return;
            }
            match res {
                Ok(_) => external_speak(
                    &book_c,
                    &ui_c,
                    &timers_c,
                    &muted_c,
                    &quitting_c,
                    started_at,
                    "メモを預かりました",
                ),
                Err(err) => {
                    eprintln!(
                        "miryam: Inkdrop への保存に失敗しました (curl exit {:?}): {}",
                        err.curl_exit, err.detail
                    );
                    external_speak(
                        &book_c,
                        &ui_c,
                        &timers_c,
                        &muted_c,
                        &quitting_c,
                        started_at,
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

/// チャット返答生成の失敗 (CLI 自体の失敗は llm.rs 側で警告済みなので二重警告しない)
fn warn_chat_once(detail: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("miryam: チャット返答の生成に失敗しました (以後この警告は抑制): {detail}");
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
        let enabled = book.inkdrop().is_some_and(|c| c.inbox_threshold > 0);
        if enabled && !quitting.get() && !muted.get() {
            run_inbox_check(
                book.clone(),
                ui.clone(),
                timers.clone(),
                muted.clone(),
                quitting.clone(),
                cache.clone(),
                last_notified.clone(),
            );
        }
        schedule_inbox_check(
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
            cache.clone(),
            last_notified.clone(),
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
        book.clone(),
        ui.clone(),
        timers.clone(),
        muted.clone(),
        quitting.clone(),
    );
    let name = book
        .inkdrop()
        .expect("見守りは inkdrop 有効時のみ")
        .book
        .clone();
    resolve_book_id(book.clone(), cache, name, move |resolved| {
        let book_id = match resolved {
            Ok(id) => id,
            Err(ResolveError::NotFound) => {
                warn_inbox_once("ノートブック解決に失敗 (名前不一致)");
                return;
            }
            Err(ResolveError::Request(err)) => {
                warn_inbox_once(&format!("curl exit {:?}: {}", err.curl_exit, err.detail));
                return;
            }
        };
        let cfg = book_r.inkdrop().expect("見守りは inkdrop 有効時のみ");
        let path = format!(
            "/notes?keyword=bookId:{}&limit={}",
            inkdrop::strip_book_prefix(&book_id),
            inkdrop::NOTES_QUERY_LIMIT
        );
        let threshold = cfg.inbox_threshold;
        inkdrop::request(cfg, "GET", &path, None, move |res| {
            if quitting_r.get() || muted_r.get() || timers_r.borrow().chat_session.is_some() {
                return; // 発行後にミュート/終了/会話中になった場合は発話もマーカーもなし
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
                    &ui_r,
                    &timers_r,
                    &format!("Inbox に {n} 件たまっています。そろそろ整理しませんか"),
                );
                *last_notified.borrow_mut() = Some(today);
            }
        });
    });
}

/// チャット関連関数が共有する状態の束 (既存のタプル引き回しの肥大化を避ける)
#[derive(Clone)]
struct ChatCtx {
    book: Rc<phrases::PhraseBook>,
    ui: Rc<ui::MascotUi>,
    timers: Rc<RefCell<Timers>>,
    muted: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    chat_book_cache: Rc<RefCell<Option<String>>>,
    started_at: Instant,
}

/// メニュー「話しかける」: 未開始なら開始、セッション中なら閉じる (トグル)
fn open_or_toggle_chat(ctx: &ChatCtx) {
    if ctx.quitting.get() {
        return;
    }
    if ctx.timers.borrow().chat_session.is_some() {
        close_chat_session(ctx);
        return;
    }
    // 定期発話の LLM リクエストが飛行中なら破棄する (キャンセル規則: speak/speak_event と同様)
    let pending = ctx.timers.borrow_mut().llm_request.take();
    if let Some(req) = pending {
        req.cancel();
    }
    ctx.timers.borrow_mut().chat_session = Some(chat::ChatSession::new(chrono::Local::now()));
    ctx.ui.open_chat();
    reset_chat_idle_timer(ctx);
}

/// 無操作タイマーの張り直し (開始・送信・返答受信で呼ぶ)
fn reset_chat_idle_timer(ctx: &ChatCtx) {
    if let Some(id) = ctx.timers.borrow_mut().chat_idle.take() {
        id.remove();
    }
    let Some(cfg) = ctx.book.chat() else { return };
    let ctx_c = ctx.clone();
    let id = glib::timeout_add_local_once(Duration::from_secs(cfg.idle_close_secs), move || {
        ctx_c.timers.borrow_mut().chat_idle = None;
        close_chat_session(&ctx_c);
    });
    ctx.timers.borrow_mut().chat_idle = Some(id);
}

/// クローズ経路の集約: キャンセル → UI 復帰 → 保存。戻り値は「保存を開始したか」。
/// Esc・メニュートグル・無操作タイムアウト・終了のすべてがここを通る
fn close_chat_session(ctx: &ChatCtx) -> bool {
    let (session, in_flight, idle) = {
        let mut t = ctx.timers.borrow_mut();
        (
            t.chat_session.take(),
            t.chat_request.take(),
            t.chat_idle.take(),
        )
    };
    let Some(session) = session else { return false };
    if let Some(id) = idle {
        id.remove();
    }
    let was_thinking = in_flight.is_some();
    if let Some(req) = in_flight {
        req.cancel(); // pending のユーザー発言はクロージャごと破棄される
    }
    ctx.ui.close_chat();
    if was_thinking {
        // 「……」が出たままにしない
        if let Some(id) = ctx.timers.borrow_mut().hide_bubble.take() {
            id.remove();
        }
        ctx.ui.hide_bubble();
    }
    if session.turns.is_empty() {
        return false;
    }
    save_chat_log(ctx, session)
}

/// 送信: 進行中キャンセル → 「……」表示 → request_raw → 返答表示 + 履歴確定
fn send_chat_message(ctx: &ChatCtx, raw_input: String) {
    if ctx.quitting.get() {
        return;
    }
    let text = raw_input.trim().to_string();
    if text.is_empty() {
        return;
    }
    let cfg = ctx
        .book
        .chat()
        .expect("チャット UI は [chat] 有効時のみ配線される");
    // セッション不在なら以降の副作用 (idle タイマー起動など) を一切起こさない。
    // 「chat_idle が Some ⇒ chat_session も Some」という不変条件をここで担保する
    if ctx.timers.borrow().chat_session.is_none() {
        return;
    }
    // 再送信: 前のリクエストをキャンセル (前の pending はクロージャごと破棄)
    if let Some(req) = ctx.timers.borrow_mut().chat_request.take() {
        req.cancel();
    }
    reset_chat_idle_timer(ctx);
    let now = phrases::Snapshot::current(ctx.started_at);
    let prompt = {
        let timers = ctx.timers.borrow();
        let session = timers
            .chat_session
            .as_ref()
            .expect("直前にセッション存在を確認済み");
        chat::build_chat_prompt(cfg, &session.turns, &text, &now)
    };
    show_text_persistent(&ctx.ui, &ctx.timers, "……");
    let ctx_c = ctx.clone();
    let user_text = text;
    let req = llm::request_raw(&cfg.command, cfg.timeout_secs, &prompt, move |raw| {
        ctx_c.timers.borrow_mut().chat_request = None;
        if ctx_c.quitting.get() {
            return;
        }
        reset_chat_idle_timer(&ctx_c); // 返答受信も「操作」扱い
        let raw_was_some = raw.is_some();
        match raw.as_deref().and_then(chat::postprocess_chat) {
            Some(reply) => {
                if let Some(session) = ctx_c.timers.borrow_mut().chat_session.as_mut() {
                    session.push_exchange(user_text, reply.clone());
                }
                let secs = chat::bubble_secs(&reply);
                show_text_for(&ctx_c.ui, &ctx_c.timers, &reply, secs);
            }
            None => {
                // CLI 自体の失敗 (raw None) は llm.rs 側で警告済みなのでここでは二重警告しない。
                // CLI は成功したが出力が空だった場合のみ、ここで警告する
                if raw_was_some {
                    warn_chat_once("出力が空でした");
                }
                // 失敗ターン: pending (user_text) はここで捨てられ、履歴に残らない
                show_text(&ctx_c.ui, &ctx_c.timers, "うまく言葉が出てきません");
            }
        }
    });
    ctx.timers.borrow_mut().chat_request = Some(req);
}

/// セッションを Inkdrop に保存する。開始できたら true。
/// 成功時は発話しない (会話の締めに毎回喋るのはノイズ)。失敗は stderr + 発話 (終了中を除く)
fn save_chat_log(ctx: &ChatCtx, session: chat::ChatSession) -> bool {
    let Some(cfg) = ctx.book.inkdrop() else {
        return false; // 起動時に「保存されません」を警告済み
    };
    let name = cfg.chat_book_name().to_string();
    let ended_at = chrono::Local::now();
    let (title, body) = chat::chat_note(&session, &ended_at);
    let ctx_c = ctx.clone();
    let name_c = name.clone();
    resolve_book_id(
        ctx.book.clone(),
        ctx.chat_book_cache.clone(),
        name,
        move |resolved| {
            let book_id = match resolved {
                Ok(id) => id,
                Err(ResolveError::NotFound) => {
                    eprintln!("miryam: ノートブック \"{name_c}\" が見つかりません");
                    if !ctx_c.quitting.get() {
                        external_speak(
                            &ctx_c.book,
                            &ctx_c.ui,
                            &ctx_c.timers,
                            &ctx_c.muted,
                            &ctx_c.quitting,
                            ctx_c.started_at,
                            &format!("ノートブック \"{name_c}\" が見つかりません"),
                        );
                    }
                    return;
                }
                Err(ResolveError::Request(err)) => {
                    eprintln!(
                        "miryam: 会話ログの保存に失敗しました (curl exit {:?}): {}",
                        err.curl_exit, err.detail
                    );
                    if !ctx_c.quitting.get() {
                        external_speak(
                            &ctx_c.book,
                            &ctx_c.ui,
                            &ctx_c.timers,
                            &ctx_c.muted,
                            &ctx_c.quitting,
                            ctx_c.started_at,
                            "会話ログを Inkdrop に残せませんでした",
                        );
                    }
                    return;
                }
            };
            let payload = inkdrop::note_payload(&book_id, &title, &body);
            let cfg = ctx_c.book.inkdrop().expect("保存は inkdrop 有効時のみ");
            let ctx_d = ctx_c.clone();
            inkdrop::request(cfg, "POST", "/notes", Some(payload), move |res| {
                if let Err(err) = res {
                    eprintln!(
                        "miryam: 会話ログの保存に失敗しました (curl exit {:?}): {}",
                        err.curl_exit, err.detail
                    );
                    if !ctx_d.quitting.get() {
                        external_speak(
                            &ctx_d.book,
                            &ctx_d.ui,
                            &ctx_d.timers,
                            &ctx_d.muted,
                            &ctx_d.quitting,
                            ctx_d.started_at,
                            "会話ログを Inkdrop に残せませんでした",
                        );
                    }
                }
            });
        },
    );
    true
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
    let Some((text, face)) = book.pick_event(event, &now) else {
        return false;
    };
    let text = phrases::substitute_placeholders(text, &now);
    let pending = timers.borrow_mut().llm_request.take();
    if let Some(req) = pending {
        req.cancel();
    }
    show_text(ui, timers, &text);
    ui.set_face(face);
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
        if !quitting.get() && !muted.get() && timers_c.borrow().chat_session.is_none() {
            scheduled_speak(&book, &ui, &timers_c, started_at);
        }
        schedule_next_speech(
            book.clone(),
            ui.clone(),
            timers_c.clone(),
            muted.clone(),
            quitting.clone(),
            started_at,
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
        if !quitting.get() && !muted.get() && timers.borrow().chat_session.is_none() {
            speak_event(&book, &ui, &timers, phrases::EventKind::Chime, started_at);
        }
        schedule_chime(
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
            started_at,
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
    chat_book_id_cache: &Rc<RefCell<Option<String>>>,
    started_at: Instant,
) {
    let chat_ctx = ChatCtx {
        book: book.clone(),
        ui: ui.clone(),
        timers: timers.clone(),
        muted: muted.clone(),
        quitting: quitting.clone(),
        chat_book_cache: chat_book_id_cache.clone(),
        started_at,
    };

    let speak_now = gio::SimpleAction::new("speak-now", None);
    {
        let (book, ui, timers, muted_r, quitting) = (
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
        );
        speak_now.connect_activate(move |_, _| {
            if quitting.get() || timers.borrow().chat_session.is_some() {
                return;
            }
            speak(&book, &ui, &timers, started_at);
            schedule_next_speech(
                book.clone(),
                ui.clone(),
                timers.clone(),
                muted_r.clone(),
                quitting.clone(),
                started_at,
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
        let ctx = chat_ctx.clone();
        quit_request.connect_activate(move |_, _| {
            if quitting.get() {
                return;
            }
            // 会話が残っていれば quitting を立てる前に保存を開始する
            let saving = close_chat_session(&ctx);
            quitting.set(true);
            let spoke = speak_event(&book, &ui, &timers, phrases::EventKind::Quit, started_at);
            let app_weak = app_weak.clone();
            if spoke || saving {
                // 保存中は台詞なしでも 2 秒待つ (localhost POST のベストエフォート完走待ち)
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
                        external_speak(&book, &ui, &timers, &muted_r, &quitting, started_at, &text);
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
            book.clone(),
            ui.clone(),
            timers.clone(),
            muted.clone(),
            quitting.clone(),
            book_id_cache.clone(),
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
                book.clone(),
                ui.clone(),
                timers.clone(),
                muted_r.clone(),
                quitting.clone(),
                cache.clone(),
                started_at,
                text,
            );
        });
    }
    app.add_action(&memo);

    // チャット: メニュー「話しかける」(トグル) + Entry の Enter / Esc
    if book.chat().is_some() {
        let chat_toggle = gio::SimpleAction::new("chat-toggle", None);
        {
            let ctx = chat_ctx.clone();
            chat_toggle.connect_activate(move |_, _| open_or_toggle_chat(&ctx));
        }
        app.add_action(&chat_toggle);
        {
            let ctx = chat_ctx.clone();
            ui.connect_chat_submitted(move |text| send_chat_message(&ctx, text));
        }
        {
            let ctx = chat_ctx.clone();
            ui.connect_chat_escape(move || {
                close_chat_session(&ctx);
            });
        }
    }

    // リンク集: リンクを既定ブラウザで開く (gtk::show_uri は 4.10 deprecated のため gio 経由)
    let open_link = gio::SimpleAction::new("open-link", Some(glib::VariantTy::STRING));
    open_link.connect_activate(move |_, param| {
        let Some(url) = param.and_then(|v| v.get::<String>()) else {
            return;
        };
        gio::AppInfo::launch_default_for_uri_async(
            &url,
            None::<&gio::AppLaunchContext>,
            None::<&gio::Cancellable>,
            move |res| {
                if let Err(e) = res {
                    eprintln!("miryam: URL を開けませんでした: {e}");
                }
            },
        );
    });
    app.add_action(&open_link);

    // リンク集: クリップボードの URL を links.toml に追記。
    // layer-shell 窓はキーボードフォーカスを持たず Wayland の selection オファーを
    // 受け取れないため、GDK のクリップボードは常に空に見える。フォーカス不要の
    // data-control プロトコルを使う wl-paste に読み取りを委譲する
    let add_link = gio::SimpleAction::new("add-link-from-clipboard", None);
    {
        let (ui, timers, quitting) = (ui.clone(), timers.clone(), quitting.clone());
        add_link.connect_activate(move |_, _| {
            if quitting.get() {
                return;
            }
            let argv: [&std::ffi::OsStr; 2] = ["wl-paste".as_ref(), "--no-newline".as_ref()];
            let subprocess = match gio::Subprocess::newv(
                &argv,
                gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE,
            ) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "miryam: wl-paste を起動できませんでした (wl-clipboard は必須です): {e}"
                    );
                    show_text(&ui, &timers, "クリップボードを読み取れませんでした");
                    return;
                }
            };
            let (ui, timers, quitting) = (ui.clone(), timers.clone(), quitting.clone());
            let subprocess_c = subprocess.clone();
            // wl-paste はローカルで即応するためタイムアウトは張らない (llm.rs とは異なり
            // ハング時も次のクリックが新プロセスを起こすだけで実害がない)
            subprocess.communicate_utf8_async(None, None::<&gio::Cancellable>, move |result| {
                if quitting.get() {
                    return;
                }
                let text = match &result {
                    Ok((stdout, _stderr)) if subprocess_c.is_successful() => {
                        stdout.as_deref().unwrap_or("").to_string()
                    }
                    // 非ゼロ終了は通常「クリップボードが空」: 「URL なし」扱いにするが、
                    // 他の失敗理由と区別できるよう stderr は残す
                    Ok((_, stderr)) => {
                        let head = stderr
                            .as_deref()
                            .and_then(|s| s.lines().next())
                            .unwrap_or("");
                        eprintln!("miryam: wl-paste が非ゼロ終了しました: {head}");
                        String::new()
                    }
                    Err(e) => {
                        eprintln!("miryam: クリップボードの読み取りに失敗しました: {e}");
                        String::new()
                    }
                };
                add_link_from_text(&ui, &timers, &text);
            });
        });
    }
    app.add_action(&add_link);

    // ドラッグで動かした位置を既定 (右下) に戻す
    let reset_position = gio::SimpleAction::new("reset-position", None);
    {
        let ui = ui.clone();
        reset_position.connect_activate(move |_, _| ui.reset_position());
    }
    app.add_action(&reset_position);

    // リンク集: 指定 URL のリンクを削除 (メニュー「リンクを削除」から)
    let remove_link = gio::SimpleAction::new("remove-link", Some(glib::VariantTy::STRING));
    {
        let (ui, timers, quitting) = (ui.clone(), timers.clone(), quitting.clone());
        remove_link.connect_activate(move |_, param| {
            if quitting.get() {
                return;
            }
            let Some(url) = param.and_then(|v| v.get::<String>()) else {
                return;
            };
            match links::remove_link(&links::links_path(), &url) {
                Ok(Some(label)) => show_text(&ui, &timers, &format!("{label} を削除しました")),
                // メニュー表示後に links.toml が変わった場合など: 黙って何もしない
                Ok(None) => {}
                Err(e) => {
                    eprintln!("miryam: {e:#}");
                    show_text(&ui, &timers, "links.toml を更新できませんでした");
                }
            }
        });
    }
    app.add_action(&remove_link);

    // リンク集サブメニュー: 右クリックのたびに links.toml を読み直す
    {
        let (ui_c, timers_c) = (ui.clone(), timers.clone());
        ui.connect_menu(book.chat().is_some(), move || {
            match links::load(&links::links_path()) {
                Ok(list) => links::build_submenu(&list),
                Err(e) => {
                    eprintln!("miryam: {e:#}");
                    show_text(&ui_c, &timers_c, "links.toml が不正です");
                    links::build_submenu(&[])
                }
            }
        });
    }
}

/// クリップボードのテキストを検証して links.toml に追記する。
/// 結果は吹き出しで知らせる (成功 / URL でない / 登録済み / 失敗)
fn add_link_from_text(ui: &Rc<ui::MascotUi>, timers: &Rc<RefCell<Timers>>, text: &str) {
    let Some(url) = links::parse_http_url(text) else {
        show_text(ui, timers, "クリップボードに URL がありません");
        return;
    };
    let path = links::links_path();
    let existing = match links::load(&path) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("miryam: {e:#}");
            show_text(ui, timers, "links.toml が不正です");
            return;
        }
    };
    if existing.iter().any(|l| l.url == url) {
        show_text(ui, timers, "登録済みです");
        return;
    }
    let label = links::label_of(&url);
    let link = links::Link {
        label: label.clone(),
        url,
    };
    match links::append_link(&path, &link) {
        Ok(()) => show_text(ui, timers, &format!("{label} を追加しました")),
        Err(e) => {
            eprintln!("miryam: {e:#}");
            show_text(ui, timers, "links.toml への書き込みに失敗しました");
        }
    }
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
