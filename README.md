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
- キャラを左ドラッグ: 位置を移動 (モニタ内にクランプ。位置は保存されず、メニュー「位置をリセット」か再起動で右下に戻ります)
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

## ニュースダイジェスト (任意)

`phrases.toml` に `[news]` セクションを書くと、定期的にニュースを取得して LLM でダイジェストにまとめます (既定は無効)。`[llm]` セクションが必須です。

```toml
[news]
# feeds = ["https://www.nhk.or.jp/rss/news/cat0.xml"]  # 省略時のデフォルト。RSS でも通常ページ URL でも可
# interval_mins = 60        # 取得間隔 (分)。15〜1440
# max_kb_per_feed = 16      # 1 ソースあたり LLM に渡すテキスト上限 (KiB)。1〜64
# focus = "テック・AI 関連を重点的に"  # どういう傾向のニュースを重点的に知りたいか (自由記述)
```

- 起動 30 秒後と interval_mins ごとに全ソースを取得し、タグを除去したテキストを `[llm]` のコマンドに渡して 1 ページ程度のダイジェストを作ります
- できあがると吹き出しで一言知らせ、右クリックメニュー「ニュースを見る」で全文を読めます (Esc で閉じる)
- `focus` を書くと、その関心に沿って取捨選択・重点化されます。ソース自体を絞りたい場合は `feeds` を指定してください
- 全ソースの取得や要約に失敗すると「ニュースが取れませんでした」と一言だけ知らせます (詳細は stderr)。一部の失敗は残りのソースだけで続行します
- ミュート中・会話中は知らせません (ダイジェストの更新は裏で続きます)
- 要約のタイムアウトは `[llm] timeout_secs` に従います。大きめのフィードを扱う場合は延ばしてください
- glib のタイマーはサスペンド中に進まないため、次回取得はその分遅れます (他機能と同じ注意)

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
  (ラベルは「ホスト + パス」(例: github.com/owner/repo)。変更したいときは links.toml を直接編集してください)
- 「リンクを削除」サブメニューから個別に削除できます (確認なしで即削除、吹き出しで通知)
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

- 定期発話の間隔は `[speech]` セクションで調整できます (未指定は 30〜90 秒のランダム):

  ```toml
  [speech]
  interval_min_secs = 15   # 下限 (5 以上)
  interval_max_secs = 45   # 上限 (下限以上)
  ```

- キャラクター画像は「スキン」として差し替えられます。`~/.config/miryam/skins/<名前>/character.png` に透過 PNG (400x600 推奨) を置き、`phrases.toml` で選択します:

  ```toml
  [skin]
  name = "mychar"
  ```

  `[skin]` を指定しない場合は、従来の `~/.config/miryam/character.png` (後方互換)、それも無ければ内蔵の仮画像が使われます。指定したスキンの画像が読めない場合は起動エラーになります。

- リポジトリには標準スキン「アーシャ」を同梱しています (`assets/skins/asha/`、立ち絵 + 表情/ポーズ差分 15 種、ChatGPT による生成画像)。使うには設定ディレクトリへコピーして `[skin]` で選択します:

  ```bash
  mkdir -p ~/.config/miryam/skins
  cp -r assets/skins/asha ~/.config/miryam/skins/
  ```

  同梱辞書の `face` 名はこのスキンの表情差分に合わせてあります。

配置したファイルが不正な場合 (TOML の構文エラー、画像のデコード失敗など) はフォールバックせず起動エラーになります。自動起動で反映されない場合は `cargo run` を手動実行してエラーメッセージを確認してください。

### 表情差分

台詞グループに `face` を指定すると、その台詞の表示中だけキャラ画像が
`~/.config/miryam/skins/<name>/character-<face>.png` に切り替わります
(吹き出しが消えると通常に戻ります)。

```toml
[[group]]
cpu = ["high"]
face = "troubled"
phrases = ["ファンが頑張っていますね"]
```

- 画像が無い表情は通常表情で表示されます (エラーにはなりません)
- 命名・キャンバス規約 (400x600・透過・アンカー統一) は通常画像と同じです
- 同梱辞書は happy / troubled / sleepy を使います。ローカル辞書は同梱辞書を
  置き換えるため、表情を使うには自分の phrases.toml に `face` を追記してください

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
- キャラ画像はアニメーションなし (表情差分による静止画切り替えのみ)
- ドラッグで動かした位置は保存されない (再起動で右下に戻る)

## 次に実装予定の機能

1. 触り反応 (キャラをクリック・撫でたときの専用反応)
2. 選択肢バルーン (吹き出しからの選択操作)
3. 相方キャラ (2 体目のマスコット)
