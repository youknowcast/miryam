use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk4 as gtk;
use gtk::{gio, glib, prelude::*};
use serde::Deserialize;

use crate::phrases::Snapshot;
use crate::system::{CpuLevel, MemLevel};

const DEFAULT_PERSONA: &str = "あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。\n以下の状況を踏まえて、画面の前のユーザーにかける一言を日本語で 1 文だけ出力してください。\n40 文字以内。軽い挨拶・気遣い・観察など。絵文字・引用符・前置き・説明は不要です。";

fn default_command() -> Vec<String> {
    vec!["claude".to_string(), "-p".to_string()]
}

fn default_probability() -> f64 {
    0.2
}

fn default_timeout() -> u64 {
    30
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    #[serde(default = "default_command")]
    command: Vec<String>,
    #[serde(default = "default_probability")]
    pub probability: f64,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    prompt: Option<String>,
}

impl LlmConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.command.is_empty() {
            anyhow::bail!("[llm] command が空です");
        }
        if !(0.0..=1.0).contains(&self.probability) {
            anyhow::bail!("[llm] probability は 0.0〜1.0 で指定してください");
        }
        if self.timeout_secs == 0 {
            anyhow::bail!("[llm] timeout_secs は 1 以上を指定してください");
        }
        Ok(())
    }
}

/// ペルソナ指示 (既定 or config.prompt) + 状況行からプロンプトを組み立てる
pub fn build_prompt(config: &LlmConfig, now: &Snapshot) -> String {
    let persona = config.prompt.as_deref().unwrap_or(DEFAULT_PERSONA);
    format!(
        "{persona}\n状況: 時間帯={}, 曜日={}, CPU={}, メモリ={}, 連続稼働={}時間",
        crate::phrases::time_band_name(now.hour),
        weekday_name(now.weekday),
        cpu_name(now.cpu),
        mem_name(now.mem),
        now.uptime.as_secs() / 3600,
    )
}

/// CLI 出力から台詞を抽出する。trim → 最初の非空行 → 60 文字切り詰め。空なら None
pub fn postprocess(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(60).collect())
}

/// 進行中の LLM リクエストのハンドル。cancel() すると結果は破棄され on_done は呼ばれない
pub struct LlmRequest {
    cancellable: gio::Cancellable,
    cancelled: Rc<Cell<bool>>,
}

impl LlmRequest {
    pub fn cancel(&self) {
        self.cancelled.set(true);
        self.cancellable.cancel();
    }
}

fn warn_once(detail: &str) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("miryam: LLM 台詞の生成に失敗しました (辞書にフォールバック): {detail}");
    }
}

/// CLI を非同期実行し、完了時に on_done をメインループ上で呼ぶ。
/// 失敗 (spawn 失敗・非ゼロ終了・タイムアウト・空出力) は on_done(None)。
/// 呼び出し元が cancel() した場合は on_done を呼ばない。
pub fn request_phrase(
    config: &LlmConfig,
    prompt: &str,
    on_done: impl FnOnce(Option<String>) + 'static,
) -> LlmRequest {
    let cancellable = gio::Cancellable::new();
    let cancelled = Rc::new(Cell::new(false));
    let request = LlmRequest {
        cancellable: cancellable.clone(),
        cancelled: cancelled.clone(),
    };

    let mut argv: Vec<&std::ffi::OsStr> = config.command.iter().map(|s| s.as_ref()).collect();
    argv.push(prompt.as_ref());

    let subprocess = match gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE,
    ) {
        Ok(p) => p,
        Err(err) => {
            warn_once(&err.to_string());
            on_done(None);
            return request;
        }
    };

    // タイムアウト: cancel + kill。スロットは「発火時に自分で None にする」既存不変条件に従う
    let timeout_slot: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let id = glib::timeout_add_local_once(std::time::Duration::from_secs(config.timeout_secs), {
        let slot = timeout_slot.clone();
        let cancellable = cancellable.clone();
        let subprocess = subprocess.clone();
        move || {
            slot.borrow_mut().take();
            cancellable.cancel();
            subprocess.force_exit();
        }
    });
    *timeout_slot.borrow_mut() = Some(id);

    let subprocess_c = subprocess.clone();
    subprocess.communicate_utf8_async(None, Some(&cancellable), move |result| {
        if let Some(id) = timeout_slot.borrow_mut().take() {
            id.remove();
        }
        if cancelled.get() {
            return; // 呼び出し元キャンセル: 何もしない (on_done は drop される)
        }
        match result {
            Ok((stdout, stderr)) if subprocess_c.is_successful() => {
                let text = stdout.as_deref().unwrap_or("");
                match postprocess(text) {
                    Some(phrase) => on_done(Some(phrase)),
                    None => {
                        warn_once("出力が空でした");
                        let _ = stderr;
                        on_done(None);
                    }
                }
            }
            Ok((_, stderr)) => {
                let head = stderr
                    .as_deref()
                    .and_then(|s| s.lines().next())
                    .unwrap_or("")
                    .to_string();
                warn_once(&format!("CLI が失敗しました: {head}"));
                on_done(None);
            }
            Err(err) => {
                warn_once(&err.to_string());
                on_done(None);
            }
        }
    });

    request
}

fn weekday_name(w: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match w {
        Mon => "mon",
        Tue => "tue",
        Wed => "wed",
        Thu => "thu",
        Fri => "fri",
        Sat => "sat",
        Sun => "sun",
    }
}

fn cpu_name(c: CpuLevel) -> &'static str {
    match c {
        CpuLevel::Idle => "idle",
        CpuLevel::Normal => "normal",
        CpuLevel::High => "high",
    }
}

fn mem_name(m: MemLevel) -> &'static str {
    match m {
        MemLevel::Normal => "normal",
        MemLevel::High => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_snapshot() -> Snapshot {
        Snapshot {
            hour: 6,
            weekday: chrono::Weekday::Mon,
            month: 6,
            day: 15,
            uptime: Duration::from_secs(2 * 3600 + 120),
            cpu: CpuLevel::Idle,
            mem: MemLevel::High,
        }
    }

    #[test]
    fn llm_config_defaults() {
        let cfg: LlmConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.command, vec!["claude", "-p"]);
        assert_eq!(cfg.probability, 0.2);
        assert_eq!(cfg.timeout_secs, 30);
        assert!(cfg.prompt.is_none());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn llm_config_validation_errors() {
        let bad: LlmConfig = toml::from_str("probability = 1.5").unwrap();
        assert!(bad.validate().is_err());
        let bad: LlmConfig = toml::from_str("probability = -0.1").unwrap();
        assert!(bad.validate().is_err());
        let bad: LlmConfig = toml::from_str("command = []").unwrap();
        assert!(bad.validate().is_err());
        let bad: LlmConfig = toml::from_str("timeout_secs = 0").unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn llm_config_rejects_unknown_keys() {
        assert!(toml::from_str::<LlmConfig>("model = \"opus\"").is_err());
    }

    #[test]
    fn build_prompt_uses_default_persona_and_context() {
        let cfg: LlmConfig = toml::from_str("").unwrap();
        let p = build_prompt(&cfg, &test_snapshot());
        assert!(p.starts_with("あなたはデスクトップ右下に常駐する小さなマスコット「miryam」です。"));
        assert!(p.ends_with("状況: 時間帯=morning, 曜日=mon, CPU=idle, メモリ=high, 連続稼働=2時間"));
    }

    #[test]
    fn build_prompt_custom_persona() {
        let cfg: LlmConfig = toml::from_str(r#"prompt = "俳句で返して""#).unwrap();
        let p = build_prompt(&cfg, &test_snapshot());
        assert!(p.starts_with("俳句で返して\n状況: "));
    }

    #[test]
    fn postprocess_picks_first_nonempty_line() {
        assert_eq!(
            postprocess("  \n\n  こんにちは  \n二行目").as_deref(),
            Some("こんにちは")
        );
    }

    #[test]
    fn postprocess_truncates_to_60_chars() {
        let long = "あ".repeat(70);
        let got = postprocess(&long).unwrap();
        assert_eq!(got.chars().count(), 60);
    }

    #[test]
    fn postprocess_empty_is_none() {
        assert!(postprocess("").is_none());
        assert!(postprocess("  \n \n").is_none());
    }

    use std::sync::Mutex;

    static MAIN_CONTEXT_LOCK: Mutex<()> = Mutex::new(());

    fn run_request(config_toml: &str, prompt: &str) -> Option<String> {
        let _lock = MAIN_CONTEXT_LOCK.lock().unwrap();
        let cfg: LlmConfig = toml::from_str(config_toml).unwrap();
        let ctx = glib::MainContext::default();
        let _guard = ctx.acquire().unwrap();
        let ml = glib::MainLoop::new(None, false);
        let result: Rc<RefCell<Option<Option<String>>>> = Rc::new(RefCell::new(None));
        let (ml_c, result_c) = (ml.clone(), result.clone());
        let _req = request_phrase(&cfg, prompt, move |r| {
            *result_c.borrow_mut() = Some(r);
            ml_c.quit();
        });
        ml.run();
        let got = result.borrow_mut().take();
        got.expect("on_done が呼ばれていない")
    }

    #[test]
    fn request_phrase_returns_cli_output() {
        let got = run_request(r#"command = ["echo", "こんにちは"]"#, "prompt");
        // echo は引数を空白連結で 1 行に出力するため "こんにちは prompt" になる
        assert_eq!(got.as_deref(), Some("こんにちは prompt"));
    }

    #[test]
    fn request_phrase_times_out() {
        let started = std::time::Instant::now();
        let got = run_request(
            "command = [\"bash\", \"-c\", \"sleep 60\"]\ntimeout_secs = 1",
            "prompt",
        );
        let elapsed = started.elapsed();
        assert!(got.is_none(), "タイムアウトは None のはず");
        assert!(elapsed.as_secs() >= 1, "即時失敗ではなくタイムアウト経路を通るはず");
        assert!(elapsed.as_secs() < 10, "1 秒タイムアウトが効いていない");
    }

    #[test]
    fn cancelled_request_never_calls_on_done() {
        let _lock = MAIN_CONTEXT_LOCK.lock().unwrap();
        let cfg: LlmConfig =
            toml::from_str(r#"command = ["bash", "-c", "sleep 2"]"#).unwrap();
        let ctx = glib::MainContext::default();
        let _guard = ctx.acquire().unwrap();
        let ml = glib::MainLoop::new(None, false);
        let called = Rc::new(Cell::new(false));

        let called_c = called.clone();
        let req = request_phrase(&cfg, "prompt", move |_| {
            called_c.set(true);
        });

        // 200ms 後にキャンセルし、その後 1.5 秒でループを抜けて未呼び出しを確認
        glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
            req.cancel();
        });
        let ml_c = ml.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            ml_c.quit();
        });
        ml.run();
        assert!(!called.get(), "cancel されたリクエストの on_done は呼ばれないはず");
    }
}
