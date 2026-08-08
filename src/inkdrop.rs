use anyhow::bail;
use gtk4 as gtk;
use gtk::gio;
use serde::Deserialize;

fn default_port() -> u16 {
    19840
}

fn default_threshold() -> u32 {
    10
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InkdropConfig {
    #[serde(default = "default_port")]
    port: u16,
    username: String,
    password: String,
    pub book: String,
    #[serde(default = "default_threshold")]
    pub inbox_threshold: u32,
}

impl InkdropConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.username.is_empty() {
            bail!("[inkdrop] username が空です");
        }
        if self.username.contains(':') {
            bail!("[inkdrop] username に : は使えません (curl の認証区切りと衝突)");
        }
        if self.password.is_empty() {
            bail!("[inkdrop] password が空です");
        }
        if self.book.is_empty() {
            bail!("[inkdrop] book が空です");
        }
        if self.port == 0 {
            bail!("[inkdrop] port は 1 以上を指定してください");
        }
        if self.inbox_threshold > 100 {
            bail!("[inkdrop] inbox_threshold は 100 以下です (0 で見守り無効)");
        }
        Ok(())
    }
}

pub const NOTES_QUERY_LIMIT: usize = 100;

/// 先頭の非空行 60 文字をタイトルに、全文 + 出所メタをボディにする
pub fn capture_note(text: &str, date: &str) -> (String, String) {
    let title: String = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("メモ")
        .chars()
        .take(60)
        .collect();
    let body = format!("{text}\n\nSource: miryam-ctl\nCaptured: {date}");
    (title, body)
}

/// POST /notes のボディ JSON を生成する
pub fn note_payload(book_id: &str, title: &str, body: &str) -> String {
    serde_json::json!({
        "doctype": "markdown",
        "bookId": book_id,
        "status": "none",
        "share": "private",
        "title": title,
        "body": body,
    })
    .to_string()
}

/// GET /books 応答から名前完全一致の _id を返す。bool は「同名が複数あった」
pub fn find_book_id(books_json: &str, name: &str) -> Option<(String, bool)> {
    let v: serde_json::Value = serde_json::from_str(books_json).ok()?;
    let arr = v.as_array()?;
    let mut hits = arr
        .iter()
        .filter(|b| b.get("name").and_then(|n| n.as_str()) == Some(name));
    let first = hits.next()?.get("_id")?.as_str()?.to_string();
    Some((first, hits.next().is_some()))
}

/// "book:XXX" → "XXX" (keyword=bookId: 用)
pub fn strip_book_prefix(id: &str) -> &str {
    id.strip_prefix("book:").unwrap_or(id)
}

/// GET /notes 応答 (配列) の件数。配列でなければ None
pub fn count_notes(notes_json: &str) -> Option<usize> {
    serde_json::from_str::<serde_json::Value>(notes_json)
        .ok()?
        .as_array()
        .map(|a| a.len())
}

/// 見守りの発話判定: しきい値有効 かつ 件数到達 かつ 今日未通知
pub fn should_notify(
    count: usize,
    threshold: u32,
    last_notified: Option<chrono::NaiveDate>,
    today: chrono::NaiveDate,
) -> bool {
    threshold > 0 && count >= threshold as usize && last_notified != Some(today)
}

/// 100 件打ち切り表示 ("100+")
pub fn format_count(n: usize, limit: usize) -> String {
    if n >= limit {
        format!("{limit}+")
    } else {
        n.to_string()
    }
}

/// curl 実行の失敗。curl_exit: 7=接続不可, 22=HTTP エラー, 28=タイムアウト
#[derive(Debug)]
pub struct RequestError {
    pub curl_exit: Option<i32>,
    pub detail: String,
}

/// Inkdrop Local Server へ curl でリクエストする (argv 固定、シェル不経由)。
/// 完了時 on_done がメインループ上で呼ばれる。キャンセル機構は持たない (短時間・冪等)
pub fn request(
    cfg: &InkdropConfig,
    method: &str,
    path_and_query: &str,
    body: Option<String>,
    on_done: impl FnOnce(Result<String, RequestError>) + 'static,
) {
    let url = format!("http://127.0.0.1:{}{}", cfg.port, path_and_query);
    let auth = format!("{}:{}", cfg.username, cfg.password);
    let mut argv_owned: Vec<String> = vec![
        "curl".into(),
        "-sf".into(),
        "-m".into(),
        "10".into(),
        "-u".into(),
        auth,
        "-X".into(),
        method.into(),
    ];
    if body.is_some() {
        argv_owned.push("-H".into());
        argv_owned.push("Content-Type: application/json".into());
        argv_owned.push("--data-binary".into());
        argv_owned.push("@-".into());
    }
    argv_owned.push(url);
    let argv: Vec<&std::ffi::OsStr> = argv_owned.iter().map(|s| s.as_ref()).collect();

    let mut flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE;
    if body.is_some() {
        flags |= gio::SubprocessFlags::STDIN_PIPE;
    }
    let subprocess = match gio::Subprocess::newv(&argv, flags) {
        Ok(p) => p,
        Err(err) => {
            on_done(Err(RequestError {
                curl_exit: None,
                detail: err.to_string(),
            }));
            return;
        }
    };
    let sp = subprocess.clone();
    subprocess.communicate_utf8_async(body, gio::Cancellable::NONE, move |result| {
        match result {
            Ok((stdout, _stderr)) if sp.is_successful() => {
                on_done(Ok(stdout.as_deref().unwrap_or("").to_string()));
            }
            Ok((_, _)) => {
                let code = sp.exit_status();
                on_done(Err(RequestError {
                    curl_exit: Some(code),
                    detail: format!("curl exit {code}"),
                }));
            }
            Err(err) => on_done(Err(RequestError {
                curl_exit: None,
                detail: err.to_string(),
            })),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(s: &str) -> InkdropConfig {
        toml::from_str(s).unwrap()
    }

    const MIN: &str = "username = \"u\"\npassword = \"p\"\nbook = \"Inbox\"\n";

    #[test]
    fn config_defaults_and_ok() {
        let c = cfg(MIN);
        assert!(c.validate().is_ok());
        assert_eq!(c.inbox_threshold, 10);
        assert_eq!(c.book, "Inbox");
    }

    #[test]
    fn config_validation_errors() {
        for bad in [
            "username = \"\"\npassword = \"p\"\nbook = \"b\"\n",
            "username = \"u:x\"\npassword = \"p\"\nbook = \"b\"\n",
            "username = \"u\"\npassword = \"\"\nbook = \"b\"\n",
            "username = \"u\"\npassword = \"p\"\nbook = \"\"\n",
            "username = \"u\"\npassword = \"p\"\nbook = \"b\"\nport = 0\n",
            "username = \"u\"\npassword = \"p\"\nbook = \"b\"\ninbox_threshold = 101\n",
        ] {
            assert!(cfg(bad).validate().is_err(), "{bad:?} はエラーのはず");
        }
    }

    #[test]
    fn config_rejects_unknown_keys() {
        assert!(toml::from_str::<InkdropConfig>(&format!("{MIN}token = \"x\"\n")).is_err());
    }

    #[test]
    fn capture_note_builds_title_and_meta() {
        let (title, body) = capture_note("  \n一行目のメモ\n二行目", "2026-08-08");
        assert_eq!(title, "一行目のメモ");
        assert!(body.starts_with("  \n一行目のメモ\n二行目"));
        assert!(body.ends_with("Source: miryam-ctl\nCaptured: 2026-08-08"));
    }

    #[test]
    fn capture_note_truncates_title_to_60_chars() {
        let long = "あ".repeat(70);
        let (title, _) = capture_note(&long, "2026-08-08");
        assert_eq!(title.chars().count(), 60);
    }

    #[test]
    fn note_payload_escapes_and_sets_fields() {
        let p = note_payload("book:JIRjBz3s", "ti\"tle", "line1\nline2\\end 日本語");
        let v: serde_json::Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v["doctype"], "markdown");
        assert_eq!(v["status"], "none");
        assert_eq!(v["share"], "private");
        assert_eq!(v["bookId"], "book:JIRjBz3s");
        assert_eq!(v["title"], "ti\"tle");
        assert_eq!(v["body"], "line1\nline2\\end 日本語");
    }

    #[test]
    fn finds_book_id_with_duplicates() {
        let json = r#"[
            {"_id":"book:aaa","name":"Inbox","parentBookId":null},
            {"_id":"book:bbb","name":"Work","parentBookId":null},
            {"_id":"book:ccc","name":"Inbox","parentBookId":"book:bbb"}
        ]"#;
        assert_eq!(
            find_book_id(json, "Inbox"),
            Some(("book:aaa".to_string(), true))
        );
        assert_eq!(
            find_book_id(json, "Work"),
            Some(("book:bbb".to_string(), false))
        );
        assert_eq!(find_book_id(json, "None"), None);
        assert_eq!(find_book_id("{}", "Inbox"), None, "非配列は None");
        assert_eq!(find_book_id("not json", "Inbox"), None);
    }

    #[test]
    fn strips_book_prefix() {
        assert_eq!(strip_book_prefix("book:JIRjBz3s"), "JIRjBz3s");
        assert_eq!(strip_book_prefix("JIRjBz3s"), "JIRjBz3s");
    }

    #[test]
    fn counts_notes() {
        assert_eq!(count_notes("[]"), Some(0));
        assert_eq!(count_notes(r#"[{"_id":"note:1"},{"_id":"note:2"}]"#), Some(2));
        assert_eq!(count_notes(r#"{"error":true}"#), None);
        assert_eq!(count_notes("broken"), None);
    }

    #[test]
    fn should_notify_gates() {
        use chrono::NaiveDate;
        let today = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 8, 7).unwrap();
        assert!(should_notify(10, 10, None, today), "しきい値ちょうどで発火");
        assert!(!should_notify(9, 10, None, today));
        assert!(!should_notify(10, 0, None, today), "0 は無効");
        assert!(!should_notify(10, 10, Some(today), today), "同日再通知なし");
        assert!(should_notify(10, 10, Some(yesterday), today), "翌日は再通知");
    }

    #[test]
    fn formats_count() {
        assert_eq!(format_count(99, 100), "99");
        assert_eq!(format_count(100, 100), "100+");
    }

    use gtk4 as gtk_t;
    use gtk_t::glib;
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::rc::Rc;

    /// 1 リクエストだけ応答して閉じる HTTP スタブ。返り値はポート
    fn spawn_stub(status_line: &'static str, body: &'static str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        port
    }

    fn run_request_against(port: u16) -> Result<String, RequestError> {
        let _lock = crate::test_sync::lock();
        let cfg = cfg(&format!(
            "username = \"u\"\npassword = \"p\"\nbook = \"Inbox\"\nport = {port}\n"
        ));
        let ctx = glib::MainContext::default();
        let _guard = ctx.acquire().unwrap();
        let ml = glib::MainLoop::new(None, false);
        let result: Rc<RefCell<Option<Result<String, RequestError>>>> = Rc::new(RefCell::new(None));
        let (ml_c, result_c) = (ml.clone(), result.clone());
        request(&cfg, "GET", "/books", None, move |r| {
            *result_c.borrow_mut() = Some(r);
            ml_c.quit();
        });
        ml.run();
        let got = result.borrow_mut().take();
        got.expect("on_done が呼ばれていない")
    }

    #[test]
    fn request_returns_body_on_success() {
        let port = spawn_stub("200 OK", r#"[{"_id":"book:x","name":"Inbox"}]"#);
        let got = run_request_against(port).unwrap();
        assert!(got.contains("book:x"));
    }

    #[test]
    fn request_reports_connection_refused() {
        // bind して即 drop したポート = 確実に閉じている
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let err = run_request_against(port).unwrap_err();
        assert_eq!(err.curl_exit, Some(7), "接続拒否は curl exit 7: {err:?}");
    }

    #[test]
    fn request_reports_http_error() {
        let port = spawn_stub("401 Unauthorized", "Invalid credentials");
        let err = run_request_against(port).unwrap_err();
        assert_eq!(err.curl_exit, Some(22), "-f の HTTP エラーは curl exit 22: {err:?}");
    }
}
