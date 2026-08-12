use std::time::Duration;

use anyhow::Context;

pub const DEFAULT_TIMER_MESSAGE: &str = "時間になりました";

const MAX_TIMER_SECS: u64 = 24 * 3600;

/// "25m 休憩" のようなタイマー指定を (待ち時間, メッセージ) に解析する。
/// duration は <正整数><s|m|h>、メッセージ省略時は既定文
pub fn parse_timer_spec(s: &str) -> anyhow::Result<(Duration, String)> {
    let mut parts = s.split_whitespace();
    let dur_tok = parts.next().context("duration がありません (例: 25m)")?;
    let (num, mult) = if let Some(n) = dur_tok.strip_suffix('s') {
        (n, 1)
    } else if let Some(n) = dur_tok.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = dur_tok.strip_suffix('h') {
        (n, 3600)
    } else {
        anyhow::bail!("duration の単位は s/m/h です: {dur_tok}");
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        anyhow::bail!("duration は <正整数><s|m|h> で指定してください: {dur_tok}");
    }
    let n: u64 = num
        .parse()
        .with_context(|| format!("duration が数値ではありません: {dur_tok}"))?;
    if n == 0 {
        anyhow::bail!("duration は 1 以上を指定してください");
    }
    let secs = n
        .checked_mul(mult)
        .filter(|&s| s <= MAX_TIMER_SECS)
        .with_context(|| format!("duration は 24h 以下にしてください: {dur_tok}"))?;
    let message: String = parts.collect::<Vec<_>>().join(" ");
    let message = if message.is_empty() {
        DEFAULT_TIMER_MESSAGE.to_string()
    } else {
        message
    };
    Ok((Duration::from_secs(secs), message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minutes_with_message() {
        let (d, m) = parse_timer_spec("25m 休憩").unwrap();
        assert_eq!(d, Duration::from_secs(25 * 60));
        assert_eq!(m, "休憩");
    }

    #[test]
    fn parses_seconds_with_default_message() {
        let (d, m) = parse_timer_spec("90s").unwrap();
        assert_eq!(d, Duration::from_secs(90));
        assert_eq!(m, DEFAULT_TIMER_MESSAGE);
    }

    #[test]
    fn joins_message_words_and_parses_hours() {
        let (d, m) = parse_timer_spec("1h 会議 の 時間").unwrap();
        assert_eq!(d, Duration::from_secs(3600));
        assert_eq!(m, "会議 の 時間");
    }

    #[test]
    fn accepts_max_24h() {
        assert!(parse_timer_spec("24h").is_ok());
        assert!(parse_timer_spec("86400s").is_ok());
    }

    #[test]
    fn rejects_bad_specs() {
        for bad in [
            "",
            "  ",
            "25x",
            "m",
            "0m",
            "-5m",
            "25h",
            "86401s",
            "２５m",
            "1.5h",
            "18446744073709551615h",
        ] {
            assert!(parse_timer_spec(bad).is_err(), "{bad:?} はエラーのはず");
        }
    }
}
