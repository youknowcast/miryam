//! curl サブプロセスの共通実行ラッパ。
//!
//! gio::Subprocess の生成・stdin 書き込み・成功/失敗判定という定型を 1 箇所に集約する。
//! 各機能 (Inkdrop / ニュース) は argv の組み立てと出力の解釈だけを担当する。

use gtk::gio;
use gtk4 as gtk;

/// curl 実行の失敗。
///
/// `exit`: Some = 非ゼロ終了 (curl の終了コード。7=接続不可, 22=HTTP エラー,
/// 28=タイムアウト など)。None = spawn または入出力の失敗。
#[derive(Debug)]
pub struct CurlError {
    pub exit: Option<i32>,
    pub detail: String,
}

/// `argv_owned` の curl を実行し、成功時の stdout を `on_done` に渡す。
/// `stdin` が Some なら STDIN_PIPE を開いて本文を書き込む。
/// 完了時 on_done がメインループ上で呼ばれる。キャンセル機構は持たない (短時間・冪等)。
pub fn run(
    argv_owned: Vec<String>,
    stdin: Option<String>,
    on_done: impl FnOnce(Result<String, CurlError>) + 'static,
) {
    let argv: Vec<&std::ffi::OsStr> = argv_owned.iter().map(|s| s.as_ref()).collect();
    let mut flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE;
    if stdin.is_some() {
        flags |= gio::SubprocessFlags::STDIN_PIPE;
    }
    let subprocess = match gio::Subprocess::newv(&argv, flags) {
        Ok(p) => p,
        Err(err) => {
            on_done(Err(CurlError {
                exit: None,
                detail: err.to_string(),
            }));
            return;
        }
    };
    let sp = subprocess.clone();
    subprocess.communicate_utf8_async(stdin, gio::Cancellable::NONE, move |result| match result {
        Ok((stdout, _stderr)) if sp.is_successful() => {
            on_done(Ok(stdout.as_deref().unwrap_or("").to_string()));
        }
        Ok((_, _)) => {
            let code = sp.exit_status();
            on_done(Err(CurlError {
                exit: Some(code),
                detail: format!("curl exit {code}"),
            }));
        }
        Err(err) => on_done(Err(CurlError {
            exit: None,
            detail: err.to_string(),
        })),
    });
}
