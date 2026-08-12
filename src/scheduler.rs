use std::time::Duration;

pub const SPEECH_INTERVAL_MIN_SECS: u64 = 30;
pub const SPEECH_INTERVAL_MAX_SECS: u64 = 90;
pub const BUBBLE_VISIBLE_SECS: u64 = 6;

/// 次の発話までの間隔を 30〜90 秒の一様乱数で返す
pub fn next_speech_interval() -> Duration {
    use rand::RngExt;
    let secs = rand::rng().random_range(SPEECH_INTERVAL_MIN_SECS..=SPEECH_INTERVAL_MAX_SECS);
    Duration::from_secs(secs)
}

/// 次の毎時 0 分までの残り時間 (min, sec は現在時刻の分・秒)
pub fn duration_until_next_hour(min: u32, sec: u32) -> Duration {
    let elapsed = (min * 60 + sec).min(3599);
    Duration::from_secs(u64::from(3600 - elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_within_bounds() {
        for _ in 0..200 {
            let d = next_speech_interval();
            assert!(d.as_secs() >= SPEECH_INTERVAL_MIN_SECS, "{d:?} が下限未満");
            assert!(d.as_secs() <= SPEECH_INTERVAL_MAX_SECS, "{d:?} が上限超過");
        }
    }

    #[test]
    fn duration_until_next_hour_boundaries() {
        assert_eq!(duration_until_next_hour(59, 59), Duration::from_secs(1));
        assert_eq!(duration_until_next_hour(0, 0), Duration::from_secs(3600));
        assert_eq!(duration_until_next_hour(30, 0), Duration::from_secs(1800));
        assert_eq!(duration_until_next_hour(59, 60), Duration::from_secs(1), "うるう秒でも 0 にならない");
    }
}
