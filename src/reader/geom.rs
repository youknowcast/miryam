pub const ZOOM_MIN: f64 = 0.25;
pub const ZOOM_MAX: f64 = 4.0;
/// ページの左右に置く余白 (px)
pub const MARGIN: f64 = 16.0;

pub fn clamp_zoom(z: f64) -> f64 {
    if z.is_nan() {
        return 1.0;
    }
    z.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// ビューポート幅にページ幅を合わせる倍率
pub fn fit_width_scale(page_w: f64, viewport_w: f64) -> f64 {
    if page_w <= 0.0 || viewport_w <= 0.0 {
        return 1.0;
    }
    clamp_zoom((viewport_w - 2.0 * MARGIN) / page_w)
}

/// 各ページの上端 y 座標 (ページ間に gap を挟む)
pub fn page_offsets(page_heights: &[f64], gap: f64) -> Vec<f64> {
    let mut y = 0.0;
    let mut out = Vec::with_capacity(page_heights.len());
    for h in page_heights {
        out.push(y);
        y += h + gap;
    }
    out
}

/// スクロール位置に対して一番上に見えているページ (0 始まり)
pub fn visible_page(scroll_y: f64, offsets: &[f64]) -> usize {
    if offsets.is_empty() {
        return 0;
    }
    match offsets.partition_point(|&o| o <= scroll_y) {
        0 => 0,
        n => n - 1,
    }
}

/// 正規化座標の点がどれかの矩形に入っているか
pub fn hit_test(rects: &[[f64; 4]], nx: f64, ny: f64) -> bool {
    rects
        .iter()
        .any(|r| nx >= r[0] && nx <= r[2] && ny >= r[1] && ny <= r[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_zoom_bounds() {
        assert_eq!(clamp_zoom(0.01), ZOOM_MIN);
        assert_eq!(clamp_zoom(99.0), ZOOM_MAX);
        assert_eq!(clamp_zoom(1.5), 1.5);
        assert_eq!(clamp_zoom(f64::NAN), 1.0, "NaN は等倍に落とす");
    }

    #[test]
    fn fit_width_leaves_margin() {
        // 余白 2 * MARGIN を引いた幅に合わせる
        let s = fit_width_scale(600.0, 1200.0);
        assert!((s - (1200.0 - 2.0 * MARGIN) / 600.0).abs() < 1e-9);
    }

    #[test]
    fn fit_width_with_degenerate_input_is_one() {
        assert_eq!(fit_width_scale(0.0, 1200.0), 1.0);
        assert_eq!(fit_width_scale(600.0, 0.0), 1.0);
        assert_eq!(fit_width_scale(600.0, 10.0), ZOOM_MIN, "極端に狭ければ下限");
    }

    #[test]
    fn page_offsets_accumulate_with_gap() {
        let offs = page_offsets(&[100.0, 200.0, 50.0], 10.0);
        assert_eq!(offs, vec![0.0, 110.0, 320.0]);
    }

    #[test]
    fn visible_page_picks_the_topmost_page_at_scroll() {
        let offs = page_offsets(&[100.0, 200.0, 50.0], 10.0);
        assert_eq!(visible_page(0.0, &offs), 0);
        assert_eq!(visible_page(109.0, &offs), 0);
        assert_eq!(visible_page(110.0, &offs), 1);
        assert_eq!(visible_page(9999.0, &offs), 2, "行き過ぎたら最終ページ");
        assert_eq!(visible_page(-5.0, &offs), 0, "負なら先頭");
    }

    #[test]
    fn visible_page_with_no_pages_is_zero() {
        assert_eq!(visible_page(10.0, &[]), 0);
    }

    #[test]
    fn hit_test_inside_and_outside() {
        let rects = [[0.1, 0.1, 0.5, 0.2], [0.1, 0.3, 0.9, 0.4]];
        assert!(hit_test(&rects, 0.2, 0.15));
        assert!(hit_test(&rects, 0.8, 0.35));
        assert!(!hit_test(&rects, 0.8, 0.15));
        assert!(!hit_test(&rects, 0.05, 0.05));
        assert!(!hit_test(&[], 0.5, 0.5));
    }
}
