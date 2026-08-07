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

- `~/.config/miryam/phrases.toml` — 台詞リスト。形式:

  ```toml
  phrases = [
    "おはようございます",
    "少し休憩してもよいのでは",
  ]
  ```

- `~/.config/miryam/character.png` — キャラクター画像 (透過 PNG 推奨、200x200 目安)

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
