use serde::Deserialize;

/// 既定のマーカー色
fn default_colors() -> Vec<String> {
    ["yellow", "green", "blue", "pink"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_recall_probability() -> f64 {
    0.1
}

/// 使える色名 (CSS の色として reader 側で解釈する)
const KNOWN_COLORS: [&str; 4] = ["yellow", "green", "blue", "pink"];

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
        for c in &self.colors {
            if !KNOWN_COLORS.contains(&c.as_str()) {
                anyhow::bail!("colors に未知の色名があります: {c} (使えるのは {KNOWN_COLORS:?})");
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
}
