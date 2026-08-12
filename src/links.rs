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

/// テキストが http/https の URL なら trim して返す。
/// 判定はホスト部の存在と空白の不在のみ (厳密な URL 文法検証はしない)
pub fn parse_http_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.contains(char::is_whitespace) {
        return None;
    }
    host_of(t)?;
    Some(t.to_string())
}

/// http/https URL からホスト名を取り出す (userinfo とポートは除く)
pub fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    if host.is_empty() {
        return None;
    }
    Some(host.to_string())
}

#[derive(Serialize)]
struct LinksFileOut<'a> {
    link: [&'a Link; 1],
}

/// links.toml の末尾に [[link]] ブロックを追記する (無ければ作成)。
/// 既存内容の整形・並び替えはしない
pub fn append_link(path: &Path, link: &Link) -> anyhow::Result<()> {
    let block = toml::to_string(&LinksFileOut { link: [link] })
        .context("リンクの TOML 変換に失敗しました")?;
    let mut content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("{} の作成に失敗しました", dir.display()))?;
            }
            String::new()
        }
        Err(e) => {
            return Err(e).with_context(|| format!("{} の読み込みに失敗しました", path.display()));
        }
    };
    if !content.is_empty() {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
    }
    content.push_str(&block);
    std::fs::write(path, content)
        .with_context(|| format!("{} への書き込みに失敗しました", path.display()))
}

pub const ADD_FROM_CLIPBOARD_LABEL: &str = "クリップボードの URL を追加";

/// 「リンク集」サブメニューを構築する。リンク 0 件時は追加項目のみ。
/// セクション区切りが GTK 上では区切り線として描画される
pub fn build_submenu(links: &[Link]) -> gtk4::gio::Menu {
    use gtk4::glib::prelude::*;
    let sub = gtk4::gio::Menu::new();
    if !links.is_empty() {
        let section = gtk4::gio::Menu::new();
        for l in links {
            let item = gtk4::gio::MenuItem::new(Some(&l.label), None);
            item.set_action_and_target_value(Some("app.open-link"), Some(&l.url.to_variant()));
            section.append_item(&item);
        }
        sub.append_section(None, &section);
    }
    let add = gtk4::gio::Menu::new();
    add.append(
        Some(ADD_FROM_CLIPBOARD_LABEL),
        Some("app.add-link-from-clipboard"),
    );
    sub.append_section(None, &add);
    sub
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

    #[test]
    fn parse_http_url_accepts_http_and_https() {
        assert_eq!(
            parse_http_url("  https://github.com/a  "),
            Some("https://github.com/a".to_string())
        );
        assert_eq!(
            parse_http_url("http://localhost:8080/x?q=1"),
            Some("http://localhost:8080/x?q=1".to_string())
        );
    }

    #[test]
    fn parse_http_url_rejects_non_urls() {
        assert_eq!(parse_http_url("こんにちは"), None);
        assert_eq!(parse_http_url("ftp://example.com"), None);
        assert_eq!(parse_http_url("https://"), None);
        assert_eq!(parse_http_url("https://exa mple.com"), None);
        assert_eq!(parse_http_url(""), None);
    }

    #[test]
    fn host_of_extracts_host() {
        assert_eq!(
            host_of("https://github.com/a/b"),
            Some("github.com".to_string())
        );
        assert_eq!(
            host_of("http://user@example.com:8080/x"),
            Some("example.com".to_string())
        );
        assert_eq!(host_of("https://"), None);
    }

    fn temp_links_path(test_name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "miryam-links-test-{}-{}",
            std::process::id(),
            test_name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("links.toml")
    }

    #[test]
    fn append_creates_file_and_dir() {
        let path = temp_links_path("create");
        let link = Link {
            label: "GitHub".into(),
            url: "https://github.com".into(),
        };
        append_link(&path, &link).unwrap();
        assert_eq!(load(&path).unwrap(), vec![link]);
    }

    #[test]
    fn append_preserves_existing_links() {
        let path = temp_links_path("preserve");
        let first = Link {
            label: "GitHub".into(),
            url: "https://github.com".into(),
        };
        let second = Link {
            label: "例".into(),
            url: "https://example.com".into(),
        };
        append_link(&path, &first).unwrap();
        append_link(&path, &second).unwrap();
        assert_eq!(load(&path).unwrap(), vec![first, second]);
    }

    #[test]
    fn append_escapes_special_characters() {
        let path = temp_links_path("escape");
        let link = Link {
            label: "引用\"符".into(),
            url: "https://example.com/a?q=\"x\"".into(),
        };
        append_link(&path, &link).unwrap();
        assert_eq!(load(&path).unwrap(), vec![link]);
    }

    #[test]
    fn submenu_lists_links_and_add_item() {
        use gtk4::prelude::*;
        let links = vec![
            Link {
                label: "GitHub".into(),
                url: "https://github.com".into(),
            },
            Link {
                label: "カレンダー".into(),
                url: "https://calendar.google.com".into(),
            },
        ];
        let menu = build_submenu(&links);
        // セクション 2 つ: リンク一覧 + 追加項目
        assert_eq!(menu.n_items(), 2);
        let section = menu.item_link(0, "section").unwrap();
        assert_eq!(section.n_items(), 2);
        let label = section
            .item_attribute_value(0, "label", Some(gtk4::glib::VariantTy::STRING))
            .unwrap();
        assert_eq!(label.str(), Some("GitHub"));
        let target = section
            .item_attribute_value(0, "target", Some(gtk4::glib::VariantTy::STRING))
            .unwrap();
        assert_eq!(target.str(), Some("https://github.com"));
        let action = section
            .item_attribute_value(0, "action", Some(gtk4::glib::VariantTy::STRING))
            .unwrap();
        assert_eq!(action.str(), Some("app.open-link"));
    }

    #[test]
    fn submenu_without_links_has_only_add_item() {
        use gtk4::prelude::*;
        let menu = build_submenu(&[]);
        assert_eq!(menu.n_items(), 1);
        let section = menu.item_link(0, "section").unwrap();
        assert_eq!(section.n_items(), 1);
        let label = section
            .item_attribute_value(0, "label", Some(gtk4::glib::VariantTy::STRING))
            .unwrap();
        assert_eq!(label.str(), Some(ADD_FROM_CLIPBOARD_LABEL));
    }
}
