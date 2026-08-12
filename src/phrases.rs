use crate::system;
use anyhow::Context;
use serde::Deserialize;
use std::time::{Duration, Instant};

const DEFAULT_PHRASES_TOML: &str = include_str!("../assets/phrases.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhrasesFile {
    phrases: Option<Vec<String>>,
    #[serde(default)]
    group: Vec<GroupRaw>,
    #[serde(default)]
    llm: Option<crate::llm::LlmConfig>,
    #[serde(default)]
    skin: Option<SkinConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRaw {
    time: Option<Vec<String>>,
    days: Option<Vec<String>>,
    dates: Option<Vec<String>>,
    uptime_hours: Option<u64>,
    cpu: Option<Vec<String>>,
    mem: Option<Vec<String>>,
    phrases: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkinConfig {
    name: String,
}

impl SkinConfig {
    fn validate(&self) -> anyhow::Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("[skin] name が空です");
        }
        if self.name.contains('/') || self.name.contains("..") {
            anyhow::bail!(
                "[skin] name にはディレクトリ名のみ指定できます (パス区切りや .. は不可): {}",
                self.name
            );
        }
        Ok(())
    }
}

struct Group {
    time: Option<Vec<TimeBand>>,
    days: Option<Vec<chrono::Weekday>>,
    dates: Option<Vec<MonthDay>>,
    uptime_hours: Option<u64>,
    cpu: Option<Vec<system::CpuLevel>>,
    mem: Option<Vec<system::MemLevel>>,
    phrases: Vec<String>,
}

impl GroupRaw {
    fn validate(self) -> anyhow::Result<Group> {
        if self.phrases.is_empty() {
            anyhow::bail!("phrases が空のグループがあります");
        }
        if self.uptime_hours == Some(0) {
            anyhow::bail!("uptime_hours は 1 以上を指定してください");
        }
        Ok(Group {
            time: self
                .time
                .map(|v| v.iter().map(|s| TimeBand::parse(s)).collect::<anyhow::Result<Vec<_>>>())
                .transpose()?,
            days: self
                .days
                .map(|v| v.iter().map(|s| parse_weekday(s)).collect::<anyhow::Result<Vec<_>>>())
                .transpose()?,
            dates: self
                .dates
                .map(|v| v.iter().map(|s| MonthDay::parse(s)).collect::<anyhow::Result<Vec<_>>>())
                .transpose()?,
            uptime_hours: self.uptime_hours,
            cpu: self
                .cpu
                .map(|v| v.iter().map(|s| parse_cpu_level(s)).collect::<anyhow::Result<Vec<_>>>())
                .transpose()?,
            mem: self
                .mem
                .map(|v| v.iter().map(|s| parse_mem_level(s)).collect::<anyhow::Result<Vec<_>>>())
                .transpose()?,
            phrases: self.phrases,
        })
    }
}

fn parse_cpu_level(s: &str) -> anyhow::Result<system::CpuLevel> {
    Ok(match s {
        "idle" => system::CpuLevel::Idle,
        "normal" => system::CpuLevel::Normal,
        "high" => system::CpuLevel::High,
        other => anyhow::bail!("不明な cpu 条件です: {other} (idle/normal/high)"),
    })
}

fn parse_mem_level(s: &str) -> anyhow::Result<system::MemLevel> {
    Ok(match s {
        "normal" => system::MemLevel::Normal,
        "high" => system::MemLevel::High,
        other => anyhow::bail!("不明な mem 条件です: {other} (normal/high)"),
    })
}

/// 条件評価に使う現在状態。時刻取得と評価を分離しテスト可能にする
pub struct Snapshot {
    pub hour: u32,
    pub weekday: chrono::Weekday,
    pub month: u32,
    pub day: u32,
    pub uptime: Duration,
    pub cpu: system::CpuLevel,
    pub mem: system::MemLevel,
}

impl Snapshot {
    /// 現在のローカル時刻と起動時刻から構築する
    pub fn current(started_at: Instant) -> Self {
        use chrono::{Datelike, Timelike};
        let (cpu, mem) = system::read_levels();
        let now = chrono::Local::now();
        Self {
            hour: now.hour(),
            weekday: now.weekday(),
            month: now.month(),
            day: now.day(),
            uptime: started_at.elapsed(),
            cpu,
            mem,
        }
    }
}

impl Group {
    fn matches(&self, now: &Snapshot) -> bool {
        self.time
            .as_ref()
            .is_none_or(|bands| bands.iter().any(|b| b.contains(now.hour)))
            && self.days.as_ref().is_none_or(|ds| ds.contains(&now.weekday))
            && self
                .dates
                .as_ref()
                .is_none_or(|ds| ds.iter().any(|d| d.month == now.month && d.day == now.day))
            && self
                .uptime_hours
                .is_none_or(|h| now.uptime >= Duration::from_secs(h.saturating_mul(3600)))
            && self.cpu.as_ref().is_none_or(|ls| ls.contains(&now.cpu))
            && self.mem.as_ref().is_none_or(|ls| ls.contains(&now.mem))
    }
}

pub struct PhraseBook {
    groups: Vec<Group>,
    llm: Option<crate::llm::LlmConfig>,
    skin: Option<SkinConfig>,
}

impl PhraseBook {
    pub fn from_toml_str(s: &str) -> anyhow::Result<Self> {
        let file: PhrasesFile =
            toml::from_str(s).context("phrases.toml のパースに失敗しました")?;
        let PhrasesFile {
            phrases: top_level,
            group,
            llm,
            skin,
        } = file;
        if let Some(cfg) = &llm {
            cfg.validate().context("[llm] の設定が不正です")?;
        }
        if let Some(cfg) = &skin {
            cfg.validate().context("[skin] の設定が不正です")?;
        }
        let groups = match (top_level, group.is_empty()) {
            (Some(_), false) => anyhow::bail!(
                "旧形式 (トップレベル phrases) と新形式 ([[group]]) は併用できません"
            ),
            (Some(phrases), true) => {
                if phrases.is_empty() {
                    anyhow::bail!("phrases.toml に台詞が 1 つもありません");
                }
                vec![Group {
                    time: None,
                    days: None,
                    dates: None,
                    uptime_hours: None,
                    cpu: None,
                    mem: None,
                    phrases,
                }]
            }
            (None, true) => anyhow::bail!("phrases.toml に台詞が 1 つもありません"),
            (None, false) => group
                .into_iter()
                .map(GroupRaw::validate)
                .collect::<anyhow::Result<Vec<_>>>()?,
        };
        Ok(Self { groups, llm, skin })
    }

    pub fn llm(&self) -> Option<&crate::llm::LlmConfig> {
        self.llm.as_ref()
    }

    pub fn skin(&self) -> Option<&str> {
        self.skin.as_ref().map(|s| s.name.as_str())
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

    pub fn pick(&self, now: &Snapshot) -> &str {
        use rand::RngExt;
        let mut pool: Vec<&str> = self
            .groups
            .iter()
            .filter(|g| g.matches(now))
            .flat_map(|g| g.phrases.iter().map(String::as_str))
            .collect();
        if pool.is_empty() {
            pool = self
                .groups
                .iter()
                .flat_map(|g| g.phrases.iter().map(String::as_str))
                .collect();
        }
        pool[rand::rng().random_range(0..pool.len())]
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

/// 時間帯名 (LLM プロンプト用)。しきい値は TimeBand と同一
pub(crate) fn time_band_name(hour: u32) -> &'static str {
    if TimeBand::Morning.contains(hour) {
        "morning"
    } else if TimeBand::Daytime.contains(hour) {
        "daytime"
    } else if TimeBand::Evening.contains(hour) {
        "evening"
    } else {
        "night"
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
        if !m.bytes().all(|b| b.is_ascii_digit()) || !d.bytes().all(|b| b.is_ascii_digit()) {
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
    fn rejects_invalid_toml() {
        assert!(PhraseBook::from_toml_str("this is not toml =").is_err());
    }

    #[test]
    fn rejects_empty_phrase_list() {
        assert!(PhraseBook::from_toml_str("phrases = []").is_err());
    }

    fn snap(hour: u32, weekday: chrono::Weekday, month: u32, day: u32, uptime_h: u64) -> Snapshot {
        Snapshot {
            hour,
            weekday,
            month,
            day,
            uptime: std::time::Duration::from_secs(uptime_h * 3600),
            cpu: system::CpuLevel::Normal,
            mem: system::MemLevel::Normal,
        }
    }

    #[test]
    fn pick_returns_a_defined_phrase() {
        let book = PhraseBook::from_toml_str(r#"phrases = ["x", "y", "z"]"#).unwrap();
        let now = snap(12, chrono::Weekday::Mon, 6, 15, 0);
        for _ in 0..50 {
            assert!(["x", "y", "z"].contains(&book.pick(&now)));
        }
    }

    #[test]
    fn unconditional_group_always_matches() {
        let toml = r#"
            [[group]]
            phrases = ["always"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert!(book.groups[0].matches(&snap(3, chrono::Weekday::Sun, 1, 1, 0)));
    }

    #[test]
    fn conditions_within_array_are_or() {
        let toml = r#"
            [[group]]
            time = ["morning", "night"]
            phrases = ["x"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        let g = &book.groups[0];
        assert!(g.matches(&snap(6, chrono::Weekday::Mon, 6, 15, 0)), "morning");
        assert!(g.matches(&snap(23, chrono::Weekday::Mon, 6, 15, 0)), "night");
        assert!(!g.matches(&snap(12, chrono::Weekday::Mon, 6, 15, 0)), "daytime は不一致");
    }

    #[test]
    fn conditions_across_keys_are_and() {
        let toml = r#"
            [[group]]
            time = ["morning"]
            days = ["mon"]
            phrases = ["x"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        let g = &book.groups[0];
        assert!(g.matches(&snap(6, chrono::Weekday::Mon, 6, 15, 0)));
        assert!(!g.matches(&snap(6, chrono::Weekday::Tue, 6, 15, 0)), "曜日不一致");
        assert!(!g.matches(&snap(12, chrono::Weekday::Mon, 6, 15, 0)), "時間帯不一致");
    }

    #[test]
    fn date_and_uptime_conditions_match() {
        let toml = r#"
            [[group]]
            dates = ["12-25"]
            phrases = ["xmas"]

            [[group]]
            uptime_hours = 4
            phrases = ["rest"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert!(book.groups[0].matches(&snap(0, chrono::Weekday::Fri, 12, 25, 0)));
        assert!(!book.groups[0].matches(&snap(0, chrono::Weekday::Fri, 12, 26, 0)));
        assert!(book.groups[1].matches(&snap(0, chrono::Weekday::Fri, 1, 1, 4)), "ちょうど 4h は一致");
        assert!(!book.groups[1].matches(&snap(0, chrono::Weekday::Fri, 1, 1, 3)));
    }

    #[test]
    fn pick_pools_only_matching_groups() {
        let toml = r#"
            [[group]]
            phrases = ["always"]

            [[group]]
            time = ["morning"]
            phrases = ["morning-only"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        let night = snap(23, chrono::Weekday::Mon, 6, 15, 0);
        for _ in 0..50 {
            assert_eq!(book.pick(&night), "always", "夜は always のみのはず");
        }
    }

    #[test]
    fn pick_falls_back_to_all_when_nothing_matches() {
        let toml = r#"
            [[group]]
            time = ["morning"]
            phrases = ["morning-only"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        let night = snap(23, chrono::Weekday::Mon, 6, 15, 0);
        assert_eq!(book.pick(&night), "morning-only", "空プールは全台詞にフォールバック");
    }

    #[test]
    fn embedded_default_is_valid() {
        let book = PhraseBook::from_toml_str(DEFAULT_PHRASES_TOML).unwrap();
        assert!(book.groups.len() >= 2, "新形式の複数グループのはず");
    }

    #[test]
    fn parses_legacy_format_as_one_group() {
        let book = PhraseBook::from_toml_str(r#"phrases = ["a", "b"]"#).unwrap();
        assert_eq!(book.groups.len(), 1);
        assert_eq!(book.groups[0].phrases, vec!["a", "b"]);
        assert!(book.groups[0].time.is_none());
    }

    #[test]
    fn parses_group_format() {
        let toml = r#"
            [[group]]
            phrases = ["always"]

            [[group]]
            time = ["morning", "night"]
            days = ["mon"]
            dates = ["12-25"]
            uptime_hours = 4
            phrases = ["conditional"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert_eq!(book.groups.len(), 2);
        assert!(book.groups[0].time.is_none());
        assert_eq!(
            book.groups[1].time,
            Some(vec![TimeBand::Morning, TimeBand::Night])
        );
        assert_eq!(book.groups[1].days, Some(vec![chrono::Weekday::Mon]));
        assert_eq!(book.groups[1].dates, Some(vec![MonthDay { month: 12, day: 25 }]));
        assert_eq!(book.groups[1].uptime_hours, Some(4));
    }

    #[test]
    fn rejects_mixed_formats() {
        let toml = r#"
            phrases = ["top"]

            [[group]]
            phrases = ["grouped"]
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err());
    }

    #[test]
    fn rejects_empty_file() {
        assert!(PhraseBook::from_toml_str("").is_err());
    }

    #[test]
    fn rejects_group_with_no_phrases() {
        let toml = r#"
            [[group]]
            time = ["morning"]
            phrases = []
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err());
    }

    #[test]
    fn rejects_zero_uptime_hours() {
        let toml = r#"
            [[group]]
            uptime_hours = 0
            phrases = ["x"]
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        let toml = r#"
            [[group]]
            day = ["mon"]
            phrases = ["x"]
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err(), "day (typo) は拒否されるはず");
    }

    #[test]
    fn rejects_invalid_condition_values() {
        for toml in [
            r#"
                [[group]]
                time = ["noon"]
                phrases = ["x"]
            "#,
            r#"
                [[group]]
                days = ["monday"]
                phrases = ["x"]
            "#,
            r#"
                [[group]]
                dates = ["13-01"]
                phrases = ["x"]
            "#,
        ] {
            assert!(PhraseBook::from_toml_str(toml).is_err(), "{toml} はエラーのはず");
        }
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
        for bad in ["1-1", "13-01", "12-32", "00-10", "12-00", "1225", "12-2x", "12/25", "12-+5", "+2-05"] {
            assert!(MonthDay::parse(bad).is_err(), "{bad} はエラーのはず");
        }
    }

    #[test]
    fn parses_cpu_and_mem_conditions() {
        let toml = r#"
            [[group]]
            cpu = ["idle", "high"]
            mem = ["high"]
            phrases = ["x"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert_eq!(
            book.groups[0].cpu,
            Some(vec![system::CpuLevel::Idle, system::CpuLevel::High])
        );
        assert_eq!(book.groups[0].mem, Some(vec![system::MemLevel::High]));
    }

    #[test]
    fn rejects_unknown_cpu_mem_values() {
        let cpu_bad = r#"
            [[group]]
            cpu = ["busy"]
            phrases = ["x"]
        "#;
        assert!(PhraseBook::from_toml_str(cpu_bad).is_err());
        let mem_bad = r#"
            [[group]]
            mem = ["low"]
            phrases = ["x"]
        "#;
        assert!(PhraseBook::from_toml_str(mem_bad).is_err());
    }

    #[test]
    fn cpu_and_mem_conditions_match() {
        let toml = r#"
            [[group]]
            cpu = ["high"]
            mem = ["high"]
            phrases = ["x"]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        let g = &book.groups[0];
        let base = snap(12, chrono::Weekday::Mon, 6, 15, 0);
        let both = Snapshot {
            cpu: system::CpuLevel::High,
            mem: system::MemLevel::High,
            ..base
        };
        assert!(g.matches(&both));
        let cpu_only = Snapshot {
            cpu: system::CpuLevel::High,
            ..snap(12, chrono::Weekday::Mon, 6, 15, 0)
        };
        assert!(!g.matches(&cpu_only), "mem 不一致なら AND で不成立");
        assert!(!g.matches(&snap(12, chrono::Weekday::Mon, 6, 15, 0)));
    }

    #[test]
    fn time_band_name_matches_bands() {
        assert_eq!(time_band_name(6), "morning");
        assert_eq!(time_band_name(12), "daytime");
        assert_eq!(time_band_name(18), "evening");
        assert_eq!(time_band_name(23), "night");
        assert_eq!(time_band_name(3), "night");
    }

    #[test]
    fn parses_llm_section() {
        let toml = r#"
            [[group]]
            phrases = ["x"]

            [llm]
            probability = 0.5
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert_eq!(book.llm().unwrap().probability, 0.5);
    }

    #[test]
    fn llm_section_works_with_legacy_format() {
        let toml = r#"
            phrases = ["x"]

            [llm]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert!(book.llm().is_some());
    }

    #[test]
    fn rejects_invalid_llm_config() {
        let toml = r#"
            [[group]]
            phrases = ["x"]

            [llm]
            probability = 2.0
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err());
    }

    #[test]
    fn llm_absent_is_none() {
        let book = PhraseBook::from_toml_str(r#"phrases = ["x"]"#).unwrap();
        assert!(book.llm().is_none());
    }

    #[test]
    fn parses_skin_section() {
        let toml = r#"
            [[group]]
            phrases = ["x"]

            [skin]
            name = "asha"

            [llm]
        "#;
        let book = PhraseBook::from_toml_str(toml).unwrap();
        assert_eq!(book.skin(), Some("asha"));
        assert!(book.llm().is_some(), "[llm] と共存できるはず");
    }

    #[test]
    fn rejects_empty_skin_name() {
        let toml = r#"
            [[group]]
            phrases = ["x"]

            [skin]
            name = ""
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err());
    }

    #[test]
    fn rejects_skin_name_with_path_components() {
        for name in ["a/b", "../up", "..", "skins/other"] {
            let toml = format!(
                "[[group]]\nphrases = [\"x\"]\n\n[skin]\nname = \"{name}\"\n"
            );
            assert!(
                PhraseBook::from_toml_str(&toml).is_err(),
                "{name} は拒否されるはず"
            );
        }
    }

    #[test]
    fn rejects_unknown_skin_keys() {
        let toml = r#"
            [[group]]
            phrases = ["x"]

            [skin]
            name = "asha"
            path = "/tmp"
        "#;
        assert!(PhraseBook::from_toml_str(toml).is_err());
    }

    #[test]
    fn skin_absent_is_none() {
        let book = PhraseBook::from_toml_str(r#"phrases = ["x"]"#).unwrap();
        assert!(book.skin().is_none());
    }
}
