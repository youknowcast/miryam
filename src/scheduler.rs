use std::time::Duration;

pub const SPEECH_INTERVAL_MIN_SECS: u64 = 30;
pub const SPEECH_INTERVAL_MAX_SECS: u64 = 90;
pub const BUBBLE_VISIBLE_SECS: u64 = 6;

/// 次の発話までの間隔を min..=max 秒の一様乱数で返す
pub fn next_speech_interval(min_secs: u64, max_secs: u64) -> Duration {
    use rand::RngExt;
    let secs = rand::rng().random_range(min_secs..=max_secs);
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
            let d = next_speech_interval(SPEECH_INTERVAL_MIN_SECS, SPEECH_INTERVAL_MAX_SECS);
            assert!(d.as_secs() >= SPEECH_INTERVAL_MIN_SECS, "{d:?} が下限未満");
            assert!(d.as_secs() <= SPEECH_INTERVAL_MAX_SECS, "{d:?} が上限超過");
        }
    }

    #[test]
    fn interval_honors_custom_bounds() {
        for _ in 0..100 {
            let d = next_speech_interval(15, 45);
            assert!((15..=45).contains(&d.as_secs()), "{d:?} が範囲外");
        }
        // min == max も動く (退化した一様分布)
        assert_eq!(next_speech_interval(10, 10).as_secs(), 10);
    }

    #[test]
    fn duration_until_next_hour_boundaries() {
        assert_eq!(duration_until_next_hour(59, 59), Duration::from_secs(1));
        assert_eq!(duration_until_next_hour(0, 0), Duration::from_secs(3600));
        assert_eq!(duration_until_next_hour(30, 0), Duration::from_secs(1800));
        assert_eq!(
            duration_until_next_hour(59, 60),
            Duration::from_secs(1),
            "うるう秒でも 0 にならない"
        );
    }
}
