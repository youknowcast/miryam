//! miryam のライブラリ層。
//! バイナリ (`miryam` / `miryam-reader`) はここのモジュールを共有する。

pub mod chat;
pub mod control;
pub mod inkdrop;
pub mod links;
pub mod llm;
pub mod news;
pub mod phrases;
pub mod reader;
pub mod scheduler;
pub mod system;
pub mod ui;

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
