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
}
