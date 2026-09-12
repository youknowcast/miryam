use serde::Deserialize;

/// 使える色名とその RGB (0.0〜1.0)。並び順は既定色の順序でもある。
/// 検証・既定値・描画色はすべてこの 1 箇所から導出する
const PALETTE: [(&str, (f64, f64, f64)); 4] = [
    ("yellow", (0.98, 0.90, 0.35)),
    ("green", (0.45, 0.85, 0.45)),
    ("blue", (0.45, 0.65, 0.95)),
    ("pink", (0.98, 0.55, 0.75)),
];

/// 既定のマーカー色 (PALETTE の並び順)
fn default_colors() -> Vec<String> {
    PALETTE.iter().map(|(name, _)| name.to_string()).collect()
}

fn default_recall_probability() -> f64 {
    0.1
}

/// ハーフ表示のときに画面のどちら側へ寄せるか
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HalfSide {
    #[default]
    Left,
    Right,
}

/// 発表モード (`[present]`)。未指定なら既定値 (左半分) を使う
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentConfig {
    /// ハーフ表示の配置。`left` (既定) か `right`
    #[serde(default)]
    pub half_side: HalfSide,
}

impl PresentConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        // 現状は列挙型で受けるので値の検証は不要 (未知の値はパース時に弾かれる)
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderConfig {
    /// PDF を置くフォルダ。先頭の `~/` は $HOME に展開する
    pub dir: String,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default = "default_colors")]
    pub colors: Vec<String>,
    /// 書き出し先ノートブック名 (省略時は [inkdrop] の book)
    #[serde(default)]
    pub book: Option<String>,
    #[serde(default = "default_recall_probability")]
    pub recall_probability: f64,
}

impl ReaderConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.dir.trim().is_empty() {
            anyhow::bail!("dir は空にできません");
        }
        if self.colors.is_empty() || self.colors.len() > 8 {
            anyhow::bail!("colors は 1〜8 個で指定してください");
        }
        let known: Vec<&str> = PALETTE.iter().map(|(name, _)| *name).collect();
        for c in &self.colors {
            if !known.contains(&c.as_str()) {
                anyhow::bail!("colors に未知の色名があります: {c} (使えるのは {known:?})");
            }
        }
        if !(0.0..=1.0).contains(&self.recall_probability) {
            anyhow::bail!("recall_probability は 0.0〜1.0 で指定してください");
        }
        if let Some(book) = &self.book
            && book.trim().is_empty()
        {
            anyhow::bail!("book は空にできません");
        }
        Ok(())
    }

    /// 先頭の `~/` だけを $HOME に展開する。それ以外はそのまま
    pub fn dir_path(&self) -> std::path::PathBuf {
        if let Some(rest) = self.dir.strip_prefix("~/")
            && let Ok(home) = std::env::var("HOME")
        {
            return std::path::Path::new(&home).join(rest);
        }
        std::path::PathBuf::from(&self.dir)
    }
}

/// reader 起動時に一度だけ読む設定束。`phrases.toml` の再パースを避けるため、
/// 各セクションを 1 回の `PhraseBook::load` から取り出す。
pub struct ReaderSettings {
    pub colors: Vec<String>,
    pub llm: Option<crate::llm::LlmConfig>,
    pub inkdrop: Option<crate::inkdrop::InkdropConfig>,
    /// 書き出し先ノートブック名。`[reader] book` → 無ければ `[inkdrop] book` →
    /// どちらも無ければ既定の "Inbox" (InkdropConfig::book の既定と同じ)
    pub book_name: String,
}

impl ReaderSettings {
    /// **読み込みに失敗しても PDF は開けなければならない**ので、辞書に問題があれば
    /// 各セクションを既定値へ落とす (理由は stderr に 1 回だけ出す)
    pub fn load() -> Self {
        match crate::phrases::PhraseBook::load() {
            Ok(book) => Self {
                colors: book
                    .reader()
                    .map(|c| c.colors.clone())
                    .unwrap_or_else(default_colors),
                llm: book.llm().cloned(),
                inkdrop: book.inkdrop().cloned(),
                book_name: book
                    .reader()
                    .and_then(|c| c.book.clone())
                    .or_else(|| book.inkdrop().map(|c| c.book.clone()))
                    .unwrap_or_else(|| "Inbox".to_string()),
            },
            Err(e) => {
                eprintln!("miryam-reader: 設定を読めないため既定値で続行します: {e:#}");
                Self {
                    colors: default_colors(),
                    llm: None,
                    inkdrop: None,
                    book_name: "Inbox".to_string(),
                }
            }
        }
    }
}

/// 発表モード側で使う設定を読む。
/// **読み込みに失敗しても発表はできなければならない**ので、既定値へ落とす (理由は stderr)
pub fn load_present() -> PresentConfig {
    match crate::phrases::PhraseBook::load() {
        Ok(book) => book.present().cloned().unwrap_or_default(),
        Err(e) => {
            eprintln!("miryam-reader: 設定を読めないため発表モードの既定値を使います: {e:#}");
            PresentConfig::default()
        }
    }
}

/// `[reader]` 未設定のユーザー向けの既定本棚フォルダ。
/// アプリのデータディレクトリ配下に作り、同梱のサンプル PDF を 1 つ置く。
/// これで設定なしでも「本棚」「PDF を発表する…」から試せる
pub fn prepare_default_library() -> std::path::PathBuf {
    let dir = gtk4::glib::user_data_dir().join("miryam").join("library");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("miryam: 既定の本棚フォルダを作れません ({}): {e}", dir.display());
        return dir;
    }
    let sample = dir.join("miryam-sample.pdf");
    if !sample.exists() {
        const SAMPLE: &[u8] = include_bytes!("../../assets/library/sample.pdf");
        if let Err(e) = std::fs::write(&sample, SAMPLE) {
            eprintln!("miryam: サンプル PDF を置けません ({}): {e}", sample.display());
        }
    }
    dir
}

/// 色名 → RGB (0.0〜1.0)。未知の名前は既定色 (PALETTE 先頭 = yellow) に落とす
pub fn color_rgb(name: &str) -> (f64, f64, f64) {
    PALETTE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, rgb)| *rgb)
        .unwrap_or(PALETTE[0].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ReaderConfig {
        toml::from_str(s).expect("パースできること")
    }

    #[test]
    fn defaults_are_filled() {
        let cfg = parse(r#"dir = "~/Documents/library""#);
        assert_eq!(cfg.dir, "~/Documents/library");
        assert!(!cfg.recursive);
        assert_eq!(cfg.colors, vec!["yellow", "green", "blue", "pink"]);
        assert_eq!(cfg.book, None);
        assert!((cfg.recall_probability - 0.1).abs() < f64::EPSILON);
        cfg.validate().expect("既定値は妥当");
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = toml::from_str::<ReaderConfig>(r#"dir = "/x"
unknown = 1"#)
            .expect_err("未知キーは拒否");
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn validate_rejects_bad_values() {
        let cases = [
            r#"dir = """#,
            r#"dir = "/x"
colors = []"#,
            r#"dir = "/x"
colors = ["yellow", "mauve"]"#,
            r#"dir = "/x"
recall_probability = 1.5"#,
            r#"dir = "/x"
recall_probability = -0.1"#,
        ];
        for case in cases {
            assert!(parse(case).validate().is_err(), "拒否されるべき: {case}");
        }
    }

    #[test]
    fn validate_rejects_too_many_colors() {
        let cfg = parse(
            r#"dir = "/x"
colors = ["yellow", "green", "blue", "pink", "yellow", "green", "blue", "pink", "yellow"]"#,
        );
        assert!(cfg.validate().is_err(), "9 色は多すぎる");
    }

    #[test]
    fn dir_path_expands_tilde() {
        let home = std::env::var("HOME").expect("HOME");
        let cfg = parse(r#"dir = "~/Documents/library""#);
        assert_eq!(
            cfg.dir_path(),
            std::path::Path::new(&home).join("Documents/library")
        );
    }

    #[test]
    fn dir_path_keeps_absolute_and_relative_as_is() {
        assert_eq!(
            parse(r#"dir = "/srv/pdf""#).dir_path(),
            std::path::PathBuf::from("/srv/pdf")
        );
        assert_eq!(
            parse(r#"dir = "pdf""#).dir_path(),
            std::path::PathBuf::from("pdf")
        );
        // 単体の "~" は展開しない (末尾スラッシュ無しは曖昧なので触らない)
        assert_eq!(
            parse(r#"dir = "~""#).dir_path(),
            std::path::PathBuf::from("~")
        );
    }

    #[test]
    fn color_rgb_knows_the_four_palette_colors() {
        assert_eq!(color_rgb("green"), (0.45, 0.85, 0.45));
        assert_eq!(color_rgb("blue"), (0.45, 0.65, 0.95));
        assert_eq!(color_rgb("pink"), (0.98, 0.55, 0.75));
        assert_eq!(color_rgb("yellow"), (0.98, 0.90, 0.35));
    }

    #[test]
    fn color_rgb_falls_back_to_yellow_for_unknown_names() {
        // 設定は PALETTE で検証済みなので通常は来ないが、
        // サイドカーに古い色名が残っている場合に備える
        assert_eq!(color_rgb("mauve"), color_rgb("yellow"));
    }

    #[test]
    fn present_defaults_to_left_half() {
        let cfg: PresentConfig = toml::from_str("").expect("空でも既定が入る");
        assert_eq!(cfg.half_side, HalfSide::Left);
        cfg.validate().expect("既定値は妥当");
    }

    #[test]
    fn present_reads_half_side() {
        let cfg: PresentConfig = toml::from_str(r#"half_side = "right""#).expect("パースできる");
        assert_eq!(cfg.half_side, HalfSide::Right);
    }

    #[test]
    fn present_rejects_unknown_values_and_keys() {
        assert!(toml::from_str::<PresentConfig>(r#"half_side = "center""#).is_err());
        assert!(toml::from_str::<PresentConfig>("unknown = 1").is_err());
    }
}
