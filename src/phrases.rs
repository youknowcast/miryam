use anyhow::{bail, Context};
use serde::Deserialize;

const DEFAULT_PHRASES_TOML: &str = include_str!("../assets/phrases.toml");

#[derive(Deserialize)]
struct PhrasesFile {
    phrases: Vec<String>,
}

pub struct PhraseBook {
    phrases: Vec<String>,
}

impl PhraseBook {
    pub fn from_toml_str(s: &str) -> anyhow::Result<Self> {
        let file: PhrasesFile =
            toml::from_str(s).context("phrases.toml のパースに失敗しました")?;
        if file.phrases.is_empty() {
            bail!("phrases.toml に台詞が 1 つもありません");
        }
        Ok(Self { phrases: file.phrases })
    }

    /// $XDG_CONFIG_HOME/miryam/phrases.toml があればそれを、無ければ埋め込みデフォルトを読む
    pub fn load() -> anyhow::Result<Self> {
        let user_path = gtk4::glib::user_config_dir()
            .join("miryam")
            .join("phrases.toml");
        if user_path.exists() {
            let s = std::fs::read_to_string(&user_path)
                .with_context(|| format!("{} の読み込みに失敗しました", user_path.display()))?;
            Self::from_toml_str(&s)
                .with_context(|| format!("{} が不正です", user_path.display()))
        } else {
            Self::from_toml_str(DEFAULT_PHRASES_TOML).context("埋め込みデフォルト台詞が不正です")
        }
    }

    pub fn pick(&self) -> &str {
        use rand::RngExt;
        let idx = rand::rng().random_range(0..self.phrases.len());
        &self.phrases[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_toml() {
        let book = PhraseBook::from_toml_str(r#"phrases = ["a", "b"]"#).unwrap();
        assert_eq!(book.phrases, vec!["a", "b"]);
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(PhraseBook::from_toml_str("this is not toml =").is_err());
    }

    #[test]
    fn rejects_empty_phrase_list() {
        assert!(PhraseBook::from_toml_str("phrases = []").is_err());
    }

    #[test]
    fn pick_returns_a_defined_phrase() {
        let book = PhraseBook::from_toml_str(r#"phrases = ["x", "y", "z"]"#).unwrap();
        for _ in 0..50 {
            assert!(["x", "y", "z"].contains(&book.pick()));
        }
    }

    #[test]
    fn embedded_default_is_valid() {
        let book = PhraseBook::from_toml_str(DEFAULT_PHRASES_TOML).unwrap();
        assert!(!book.phrases.is_empty());
    }
}
