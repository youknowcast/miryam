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

/// 思い出し台詞の抽選: 確率 `probability` で `remarks` から 1 本引く。
///
/// - `remarks` が空 → None (作り置きが無いので抽選自体をスキップ)
/// - `gate` (0.0〜1.0 の一様乱数) が `probability` 以上 → None (外れ)
/// - 当たり → `remarks[index % remarks.len()]`
///
/// `gate` / `index` は呼び出し側で生成して注入する (テストを決定的にするため)。
/// 優先順位は仕様書どおり「まず recall_probability で引いて、外れたら [llm] probability
/// の抽選に進む」。思い出し台詞は作り置きなので LLM 呼び出しは発生しない
pub fn pick_recall(remarks: &[String], probability: f64, gate: f64, index: usize) -> Option<String> {
    if remarks.is_empty() || gate >= probability {
        return None;
    }
    Some(remarks[index % remarks.len()].clone())
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

    /// 思い出し台詞の抽選。`gate` は 0.0〜1.0 の一様乱数、`index` は整数乱数
    /// (呼び出し側で生成し、ここでは決定的にテストできるように注入する)
    #[test]
    fn pick_recall_of_empty_remarks_is_none() {
        assert_eq!(pick_recall(&[], 0.1, 0.0, 0), None);
    }

    #[test]
    fn pick_recall_of_zero_probability_is_none() {
        assert_eq!(pick_recall(&["面白かった".into()], 0.0, 0.0, 0), None);
    }

    #[test]
    fn pick_recall_gates_on_the_probability() {
        let remarks = vec!["面白かった".to_string()];
        assert_eq!(pick_recall(&remarks, 0.1, 0.05, 0).as_deref(), Some("面白かった"));
        assert_eq!(pick_recall(&remarks, 0.1, 0.5, 0), None, "gate >= probability は外れ");
        assert_eq!(pick_recall(&remarks, 0.1, 0.1, 0), None, "gate == probability も外れ");
    }

    #[test]
    fn pick_recall_selects_by_index_without_going_out_of_bounds() {
        let remarks = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // gate は必ず当たる 0.0。index は配列長で割った余りで選ぶ
        assert_eq!(pick_recall(&remarks, 1.0, 0.0, 100).as_deref(), Some("b"));
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
