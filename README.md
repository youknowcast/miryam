# miryam

Omarchy / Hyprland / Wayland 上で動作する伺か風デスクトップマスコット。
画面右下に透過キャラクターが常駐し、30〜90 秒のランダム間隔で吹き出しに台詞を表示します。

## 必要パッケージ (Arch Linux / Omarchy)

```bash
sudo pacman -S --needed gtk4 gtk4-layer-shell
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
- キャラを右クリック → 「終了」: アプリを終了
- キャラ以外の透明部分はクリックが下のウィンドウへ素通しされます

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

## 次に実装予定の機能

1. 会話辞書 (時間帯・曜日などの条件付き台詞)
2. システム状態への反応 (CPU 負荷、バッテリーなど)
3. LLM 連携による会話 (`src/phrases.rs` の境界に非同期実装を追加)
