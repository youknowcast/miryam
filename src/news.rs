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
}
