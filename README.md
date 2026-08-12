# miryam

Omarchy / Hyprland / Wayland 上で動作する伺か風デスクトップマスコット。
画面右下に透過キャラクターが常駐し、30〜90 秒のランダム間隔で吹き出しに台詞を表示します。

## 必要パッケージ (Arch Linux / Omarchy)

```bash
sudo pacman -S --needed gtk4 gtk4-layer-shell wl-clipboard
```

Rust ツールチェーンは mise または rustup で導入してください (edition 2024 対応版)。

## 起動

```bash
cargo run
```

リリースビルド:

```bash
cargo build --release
./target/release/miryam
```

## 操作

- キャラを左クリック: 即座に別の台詞を表示
- キャラを右クリック: メニューを表示
  - 「今すぐ話す」: 即座に発話
  - 「自動発話を停止」: 定期発話と時報を止めます (クリック反応は生きたまま)
  - 「終了」: 終了の挨拶をしてから閉じます
- キャラ以外の透明部分はクリックが下のウィンドウへ素通しされます

## 外部から喋らせる (miryam-ctl)

同梱の `bin/miryam-ctl` で、起動中の miryam に D-Bus 経由で発話させられます (PATH に置くと便利です: `ln -s "$PWD/bin/miryam-ctl" ~/.local/bin/`):

```bash
miryam-ctl say "ビルドが終わりました"          # 即座に一言喋る
cargo build; miryam-ctl say "ビルド完了です"    # 長時間コマンドの完了通知に
miryam-ctl timer 25m 休憩の時間です             # タイマー: 満了時に発話 + デスクトップ通知
miryam-ctl timer 90s                            # メッセージ省略時は「時間になりました」
```

- テキストは 1 行 60 文字に整形されます。ミュート中でも喋ります (明示的な要求のため)
- duration は `<正整数><s|m|h>`、最大 24h。タイマーの一覧・キャンセルはありません (アプリ終了で消えます)
- glib のタイマーはサスペンド中に進まないため、ラップトップを閉じていた時間だけ満了が遅れます

## Inkdrop 連携 (任意)

`phrases.toml` に `[inkdrop]` セクションを書くと、Inkdrop の Local HTTP Server 経由で 2 つの機能が有効になります (既定は無効):

```toml
[inkdrop]
username = "..."          # Local Server の Basic 認証
password = "..."
book = "Inbox"            # capture 先・見守り対象のノートブック名
# port = 19840            # 省略可
# inbox_threshold = 10    # 省略可。0 で見守り無効 (最大 100)
```

```bash
miryam-ctl memo "あとで調べる: GraphQL の件数制限"   # → Inbox に即ノート作成、キャラが確認
alias memo='miryam-ctl memo'                          # シェルからの入力に便利
```

- **Quick Capture**: `miryam-ctl memo` で 1 行 (複数行も可) を Inbox にノート化します。タイトルは先頭行 60 文字、本文に `Source: miryam-ctl` と日付が付きます。失敗時はキャラが知らせ、詳細は stderr に出ます
- **Inbox 見守り**: 起動 30 秒後と 6 時間ごとに Inbox の件数を確認し、しきい値以上なら 1 日 1 回だけ「Inbox に N 件たまっています」と知らせます (ミュート中は黙ります)
- glib のタイマーはサスペンド中に進まないため、見守りタイマーもラップトップを閉じていた時間だけ次回確認が遅れます (say/timer と同じ注意)
- note 本文の上限は 1 MiB です。超過した場合は保存失敗としてキャラが「Inkdrop に届きませんでした」と知らせます
- walker 等のランチャーから 1 行入力をそのまま渡すこともできます (例: `walker --dmenu` 系が無い環境では `rofi` などでも可):
  ```bash
  miryam-ctl memo "$(rofi -dmenu -p memo)"   # rofi で 1 行入力してそのまま Inbox にキャプチャ
  ```
- Inkdrop 側の準備: 設定で Local HTTP Server を有効化 (Preferences の保存が効かない場合は Inkdrop 終了後に `~/.config/inkdrop/config.json` の `*.core.server` に `{"enabled": true, "port": 19840, "bindAddress": "127.0.0.1", "auth": {"username": "...", "password": "..."}}` を直接書く) → Inkdrop 再起動
- 認証情報が入るため `chmod 600 ~/.config/miryam/phrases.toml` を推奨します。phrases.toml を共有・公開する際は `[inkdrop]` を必ず除いてください

## 会話 (claude 経由)

右クリックメニューの「話しかける」で入力欄が開き、キャラクタと会話できます。
`phrases.toml` に `[chat]` セクションがあるときだけ有効です。

```toml
[chat]
# command = ["claude", "-p"]   # 既定値
# timeout_secs = 60            # 返答待ちの上限
# idle_close_secs = 600        # 無操作でセッション自動終了 (秒)
# prompt = "..."               # 会話用ペルソナの上書き

[inkdrop]
# ... 既存設定 ...
chat_book = "ChatLog"          # 会話ログの保存先 (省略時は book と同じ)
```

- Enter で送信、Esc・メニュー再選択・無操作 10 分でセッション終了
- セッション終了時に会話全体が 1 ノートとして Inkdrop に保存されます
  ([inkdrop] 未設定なら保存なし。保存失敗時のリトライはありません)
- 会話中は定期発話・時報は割り込みません
- アプリ終了時の保存はベストエフォートです (2 秒だけ完了を待ちます)

## リンク集

右クリックメニューの「リンク集」から、よく使う URL を既定ブラウザで開けます。
設定ファイルは `~/.config/miryam/links.toml` で、右クリックのたびに読み直されます
(編集後の再起動は不要)。

```toml
[[link]]
label = "GitHub"
url = "https://github.com/youknowcast"
```

- 「クリップボードの URL を追加」で、コピー中の URL をワンクリック登録できます
  (ラベルはホスト名。変更したいときは links.toml を直接編集してください)
- クリップボードの読み取りには `wl-paste` (wl-clipboard) が必要です
  (layer-shell 窓はフォーカスを持たず Wayland の selection を受け取れないため)
- links.toml が無い・リンク 0 件でもメニューは表示され、追加項目だけが並びます
- 不正な links.toml は吹き出しで知らせ、リンクは表示されません (起動は妨げません)

## カスタマイズ

ファイルを置くだけで差し替えられます (無ければ内蔵デフォルトを使用):

- `~/.config/miryam/phrases.toml` — 台詞辞書。条件付きグループで定義します:

  ```toml
  [[group]]                    # 条件なし = 常時候補
  phrases = ["CPUは平静です"]

  [[group]]
  time = ["morning"]           # 時間帯
  phrases = ["おはようございます"]

  [[group]]
  days = ["sat", "sun"]        # 曜日
  phrases = ["休日もお疲れさまです"]

  [[group]]
  dates = ["12-24", "12-25"]   # 月-日
  phrases = ["メリークリスマス"]

  [[group]]
  cpu = ["high"]           # CPU 負荷 (idle/normal/high)
  phrases = ["ファンが頑張っていますね"]

  [[group]]
  uptime_hours = 4             # 連続稼働 n 時間以上
  phrases = ["そろそろ休憩しませんか"]
  ```

  条件の値: `time` は `morning` (5–10時) / `daytime` (11–16時) / `evening` (17–21時) / `night` (22–4時)、`days` は `mon`〜`sun`、`dates` は `MM-DD` (ゼロ埋め 2 桁)、`cpu` は `idle` / `normal` / `high` (load1÷コア数が 10% 未満で idle、80% 以上で high)、`mem` は `normal` / `high` (空きメモリが 10% 未満で high)。同一グループ内の複数条件は AND、配列内は OR です。発話時にマッチした全グループの台詞から 1 つ選ばれ、どれもマッチしない場合は全台詞が候補になります。cpu / mem は発話の瞬間のシステム状態で判定されます。

  `event = "boot" | "quit" | "chime"` を付けたグループは通常のランダム発話から除外され、起動時 / 終了時 / 毎時 0 分にだけ使われます。台詞中の `{hour}` は現在の時 (0〜23) に置換されます。

  旧形式 (トップレベルの `phrases = [...]` のみ) も引き続き読み込めます。新旧の混在はエラーになります。

- キャラクター画像は「スキン」として差し替えられます。`~/.config/miryam/skins/<名前>/character.png` に透過 PNG (400x600 推奨) を置き、`phrases.toml` で選択します:

  ```toml
  [skin]
  name = "mychar"
  ```

  `[skin]` を指定しない場合は、従来の `~/.config/miryam/character.png` (後方互換)、それも無ければ内蔵の仮画像が使われます。指定したスキンの画像が読めない場合は起動エラーになります。

配置したファイルが不正な場合 (TOML の構文エラー、画像のデコード失敗など) はフォールバックせず起動エラーになります。自動起動で反映されない場合は `cargo run` を手動実行してエラーメッセージを確認してください。

### LLM 連携 (任意)

`phrases.toml` に `[llm]` セクションを書くと、定期発話の一部をローカルの LLM CLI が生成した台詞に差し替えます。既定は無効です。

```toml
[llm]
command = ["claude", "-p"]   # 既定。codex なら ["codex", "exec"]
probability = 0.2            # 定期発話を LLM に差し替える確率 (0.0〜1.0)
timeout_secs = 30            # 超過で辞書台詞にフォールバック
# prompt = "..."             # ペルソナ指示の差し替え (状況情報は自動で付加されます)
```

- API キーは扱いません。CLI 側でログイン済みであることが前提です (`claude` / `codex` など)
- 生成には数秒〜数十秒かかり、その分サブスクリプションの利用量を消費します
- 失敗・タイムアウト時は辞書の台詞に自動フォールバックします。キャラクターをクリックしたときは即応性を優先して常に辞書から発話します

## Hyprland で自動起動する

`~/.config/hypr/hyprland.conf` (Omarchy では `~/.config/hypr/autostart.conf`) に追記:

```conf
exec-once = /path/to/miryam/target/release/miryam
```

## 現在の制約

- Esc キーでの終了は未対応 (右クリックメニューを使用)
- 吹き出しは表示専用でクリック不可
- キャラ画像は 1 枚固定 (表情差分・アニメーションなし)
- 台詞は固定文からのランダム選択のみ
