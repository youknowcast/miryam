use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// リンク集の 1 エントリ。links.toml の [[link]] に対応する
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinksFile {
    #[serde(default)]
    link: Vec<Link>,
}

pub fn from_toml_str(s: &str) -> anyhow::Result<Vec<Link>> {
    let file: LinksFile = toml::from_str(s).context("links.toml のパースに失敗しました")?;
    for l in &file.link {
        anyhow::ensure!(!l.label.trim().is_empty(), "[[link]] label が空です");
        anyhow::ensure!(!l.url.trim().is_empty(), "[[link]] url が空です");
    }
    Ok(file.link)
}

/// $XDG_CONFIG_HOME/miryam/links.toml (phrases.toml と同じディレクトリ)
pub fn links_path() -> PathBuf {
    gtk4::glib::user_config_dir()
        .join("miryam")
        .join("links.toml")
}

/// links.toml を読む。ファイルが無いのは正常 (リンク 0 件)
pub fn load(path: &Path) -> anyhow::Result<Vec<Link>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("{} の読み込みに失敗しました", path.display()))?;
    from_toml_str(&s).with_context(|| format!("{} が不正です", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_links() {
        let toml = r#"
            [[link]]
            label = "GitHub"
            url = "https://github.com"

            [[link]]
            label = "カレンダー"
            url = "https://calendar.google.com"
        "#;
        let links = from_toml_str(toml).unwrap();
        assert_eq!(
            links,
            vec![
                Link {
                    label: "GitHub".into(),
                    url: "https://github.com".into()
                },
                Link {
                    label: "カレンダー".into(),
                    url: "https://calendar.google.com".into()
                },
            ]
        );
    }

    #[test]
    fn empty_file_is_no_links() {
        assert_eq!(from_toml_str("").unwrap(), vec![]);
    }

    #[test]
    fn missing_url_is_error() {
        let toml = r#"
            [[link]]
            label = "GitHub"
        "#;
        assert!(from_toml_str(toml).is_err());
    }

    #[test]
    fn blank_label_is_error() {
        let toml = r#"
            [[link]]
            label = " "
            url = "https://github.com"
        "#;
        assert!(from_toml_str(toml).is_err());
    }

    #[test]
    fn unknown_key_is_error() {
        let toml = r#"
            [[link]]
            label = "GitHub"
            url = "https://github.com"
            icon = "octocat"
        "#;
        assert!(from_toml_str(toml).is_err());
    }

    #[test]
    fn load_missing_file_is_empty() {
        let path = std::path::Path::new("/nonexistent/miryam-test/links.toml");
        assert_eq!(load(path).unwrap(), vec![]);
    }
}
