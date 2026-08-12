use serde::Deserialize;

pub const DEFAULT_FEED: &str = "https://www.nhk.or.jp/rss/news/cat0.xml";

fn default_feeds() -> Vec<String> {
    vec![DEFAULT_FEED.to_string()]
}

fn default_interval_mins() -> u64 {
    60
}

fn default_max_kb() -> u64 {
    16
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewsConfig {
    #[serde(default = "default_feeds")]
    pub feeds: Vec<String>,
    #[serde(default = "default_interval_mins")]
    pub interval_mins: u64,
    #[serde(default = "default_max_kb")]
    pub max_kb_per_feed: u64,
    #[serde(default)]
    pub focus: Option<String>,
}

impl NewsConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.feeds.is_empty() {
            anyhow::bail!("[news] feeds が空です (省略すればデフォルトを使います)");
        }
        for url in &self.feeds {
            if crate::links::parse_http_url(url).is_none() {
                anyhow::bail!("[news] feeds の URL が不正です: {url}");
            }
        }
        if !(15..=1440).contains(&self.interval_mins) {
            anyhow::bail!("[news] interval_mins は 15〜1440 で指定してください");
        }
        if !(1..=64).contains(&self.max_kb_per_feed) {
            anyhow::bail!("[news] max_kb_per_feed は 1〜64 で指定してください");
        }
        if self.focus.as_deref() == Some("") {
            anyhow::bail!("[news] focus は指定するなら空にできません");
        }
        Ok(())
    }
}

/// `<tag ...>...</tag>` ブロックを大文字小文字無視で丸ごと除去する。
/// 検索は ASCII 小文字化したコピーで行う (バイト位置を保つため to_lowercase は使わない)
fn remove_blocks(s: &str, tag: &str) -> String {
    let mut lower = s.to_string();
    lower.make_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(found) = lower[pos..].find(&open) {
        let start = pos + found;
        out.push_str(&s[pos..start]);
        match lower[start..].find(&close) {
            Some(end) => pos = start + end + close.len(),
            None => return out, // 閉じタグなし: 以降は捨てる
        }
    }
    out.push_str(&s[pos..]);
    out
}

/// script/style 除去 → タグ除去 → 最小エンティティデコード → 空白圧縮。
/// RSS/XML も HTML も同一経路で「LLM に渡せるテキスト」にする
pub fn strip_tags(html: &str) -> String {
    let cleaned = remove_blocks(&remove_blocks(html, "script"), "style");
    let mut text = String::with_capacity(cleaned.len());
    let mut in_tag = false;
    for c in cleaned.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' '); // タグ境界は空白にして語の癒着を防ぐ
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    // &amp; は最後 (先にやると &amp;lt; が < まで潰れる)
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 文字境界を保って先頭 max_bytes 以内に切り詰める
pub fn truncate_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> NewsConfig {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn config_defaults() {
        let c = parse("");
        assert_eq!(c.feeds, vec!["https://www.nhk.or.jp/rss/news/cat0.xml"]);
        assert_eq!(c.interval_mins, 60);
        assert_eq!(c.max_kb_per_feed, 16);
        assert!(c.focus.is_none());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn config_validate_bounds() {
        assert!(parse("interval_mins = 14").validate().is_err());
        assert!(parse("interval_mins = 15").validate().is_ok());
        assert!(parse("interval_mins = 1441").validate().is_err());
        assert!(parse("max_kb_per_feed = 0").validate().is_err());
        assert!(parse("max_kb_per_feed = 65").validate().is_err());
        assert!(parse("feeds = []").validate().is_err());
        assert!(parse(r#"feeds = ["ftp://example.com/a"]"#).validate().is_err());
        assert!(
            parse(r#"feeds = ["https://example.com/rss", "http://例.jp/x"]"#)
                .validate()
                .is_ok()
        );
        assert!(parse(r#"focus = """#).validate().is_err());
        assert!(parse(r#"focus = "テック中心""#).validate().is_ok());
    }

    #[test]
    fn strip_tags_removes_script_style_and_tags() {
        let html = r#"<html><head><STYLE>body{color:red}</STYLE>
        <script type="text/javascript">var x = "<p>fake</p>";</script></head>
        <body><h1>見出し</h1><p>本文 &amp; 続き &lt;A&gt;</p></body></html>"#;
        assert_eq!(strip_tags(html), "見出し 本文 & 続き <A>");
    }

    #[test]
    fn strip_tags_handles_rss() {
        let rss = "<rss><channel><item><title>ニュース1</title><description>概要1</description></item></channel></rss>";
        assert_eq!(strip_tags(rss), "ニュース1 概要1");
    }

    #[test]
    fn strip_tags_unclosed_script_drops_rest() {
        assert_eq!(strip_tags("前<script>alert(1)"), "前");
    }

    #[test]
    fn strip_tags_decodes_minimal_entities() {
        assert_eq!(
            strip_tags("a&nbsp;b &quot;c&quot; &#39;d&#39; &amp;lt;"),
            "a b \"c\" 'd' &lt;",
            "&amp; は最後にデコード (二重デコードしない)"
        );
    }

    #[test]
    fn truncate_respects_char_boundary() {
        assert_eq!(truncate_bytes("abcdef", 4), "abcd");
        assert_eq!(truncate_bytes("あいう", 4), "あ", "3 バイト文字の途中で切らない");
        assert_eq!(truncate_bytes("abc", 10), "abc");
    }
}
