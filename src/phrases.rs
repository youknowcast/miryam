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

#[derive(Clone, Copy, PartialEq, Debug)]
enum TimeBand {
    Morning,
    Daytime,
    Evening,
    Night,
}

impl TimeBand {
    fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "morning" => Self::Morning,
            "daytime" => Self::Daytime,
            "evening" => Self::Evening,
            "night" => Self::Night,
            other => anyhow::bail!(
                "不明な時間帯です: {other} (morning / daytime / evening / night)"
            ),
        })
    }

    fn contains(self, hour: u32) -> bool {
        match self {
            Self::Morning => (5..11).contains(&hour),
            Self::Daytime => (11..17).contains(&hour),
            Self::Evening => (17..22).contains(&hour),
            Self::Night => hour >= 22 || hour < 5,
        }
    }
}

fn parse_weekday(s: &str) -> anyhow::Result<chrono::Weekday> {
    use chrono::Weekday::*;
    Ok(match s {
        "mon" => Mon,
        "tue" => Tue,
        "wed" => Wed,
        "thu" => Thu,
        "fri" => Fri,
        "sat" => Sat,
        "sun" => Sun,
        other => anyhow::bail!("不明な曜日です: {other} (mon/tue/wed/thu/fri/sat/sun)"),
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct MonthDay {
    month: u32,
    day: u32,
}

impl MonthDay {
    fn parse(s: &str) -> anyhow::Result<Self> {
        use anyhow::Context;
        let (m, d) = s
            .split_once('-')
            .with_context(|| format!("日付は MM-DD 形式で指定してください: {s}"))?;
        if m.len() != 2 || d.len() != 2 {
            anyhow::bail!("日付は MM-DD 形式 (ゼロ埋め 2 桁) で指定してください: {s}");
        }
        let month: u32 = m.parse().with_context(|| format!("月が数値ではありません: {s}"))?;
        let day: u32 = d.parse().with_context(|| format!("日が数値ではありません: {s}"))?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            anyhow::bail!("日付が範囲外です: {s} (月 01-12, 日 01-31)");
        }
        Ok(Self { month, day })
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

    #[test]
    fn time_band_boundaries() {
        use TimeBand::*;
        for (hour, band) in [
            (0, Night),
            (4, Night),
            (5, Morning),
            (10, Morning),
            (11, Daytime),
            (16, Daytime),
            (17, Evening),
            (21, Evening),
            (22, Night),
            (23, Night),
        ] {
            assert!(band.contains(hour), "hour {hour} は {band:?} のはず");
        }
        assert!(!Morning.contains(4));
        assert!(!Night.contains(5));
        assert!(!Daytime.contains(17));
        assert!(!Evening.contains(22));
    }

    #[test]
    fn parses_time_band_names() {
        assert_eq!(TimeBand::parse("morning").unwrap(), TimeBand::Morning);
        assert_eq!(TimeBand::parse("daytime").unwrap(), TimeBand::Daytime);
        assert_eq!(TimeBand::parse("evening").unwrap(), TimeBand::Evening);
        assert_eq!(TimeBand::parse("night").unwrap(), TimeBand::Night);
        assert!(TimeBand::parse("noon").is_err());
        assert!(TimeBand::parse("Morning").is_err());
    }

    #[test]
    fn parses_weekday_names() {
        use chrono::Weekday;
        assert_eq!(parse_weekday("mon").unwrap(), Weekday::Mon);
        assert_eq!(parse_weekday("sun").unwrap(), Weekday::Sun);
        assert!(parse_weekday("monday").is_err());
        assert!(parse_weekday("MON").is_err());
    }

    #[test]
    fn parses_month_day() {
        assert_eq!(MonthDay::parse("12-25").unwrap(), MonthDay { month: 12, day: 25 });
        assert_eq!(MonthDay::parse("02-29").unwrap(), MonthDay { month: 2, day: 29 });
        for bad in ["1-1", "13-01", "12-32", "00-10", "12-00", "1225", "12-2x", "12/25"] {
            assert!(MonthDay::parse(bad).is_err(), "{bad} はエラーのはず");
        }
    }
}
