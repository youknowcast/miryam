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

/// しおりに記録すべきページ。末尾までスクロールしている場合は最終ページとみなす
/// (末尾では GtkAdjustment がクランプするため実際のスクロール位置は最終ページの開始点まで
/// 届かず、そのまま `visible_page` に渡すと 1 ページ手前を返す。これをそのまま保存すると
/// 開いて閉じるだけでしおりが 1 ページずつ後退していく)
///
/// `scroll_y > 0.0` を要求するのは、ドキュメント全体がビューポートに収まっていて
/// スクロールが一切できない (value も upper - page_size も 0) ケースを締め出すため。
/// このとき `at_end` は (0 >= 0 という理屈で) 真になり得るが、正しい答えは先頭ページ。
/// `scroll_y` は生の adjustment value ではなく、呼び出し側 (close ハンドラ) が
/// `adj.value() - geom::MARGIN` として渡す、左右余白を差し引いた後の値
pub fn bookmark_page(scroll_y: f64, offsets: &[f64], at_end: bool) -> usize {
    if at_end && scroll_y > 0.0 && !offsets.is_empty() {
        return offsets.len() - 1;
    }
    visible_page(scroll_y, offsets)
}

/// ズームを `old_z` から `new_z` に変えたとき、ページの高さのピクセル値が変わるか
/// (= GTK にリサイズが積まれ、vadjustment の `changed` が飛んでくるか)。
///
/// 幅は見ない: ページは縦の `gtk::Box` に並んでいて各ページの高さで
/// `content_height` が決まるため、vadjustment (縦スクロール) の `changed` を
/// 動かすのは高さの変化だけ。幅だけ整数値が変わっても hadjustment が動くだけで
/// vadjustment の `upper`/`page_size` はそのまま `gtk_adjustment_configure` される
/// ので `changed` は飛ばない。ここで幅も見てしまうと「`changed` を待つべきでない
/// のに待ってしまう」のとは逆の、「`changed` を待つべきなのに待たずに飛んでしまい、
/// 二度とハンドラが起きない」false positive になる (幅だけ変わるケースで判明)。
///
/// 丸めは `pages.rs` の `set_content_height` と同じ `as i32` (`scaled_heights` の
/// `trunc()` と同じ理由) に揃える
pub fn resize_queued(sizes: &[(f64, f64)], old_z: f64, new_z: f64) -> bool {
    sizes
        .iter()
        .any(|&(_, h)| (h * old_z) as i32 != (h * new_z) as i32)
}

/// 正規化座標の点がどれかの矩形に入っているか
pub fn hit_test(rects: &[[f64; 4]], nx: f64, ny: f64) -> bool {
    rects
        .iter()
        .any(|r| nx >= r[0] && nx <= r[2] && ny >= r[1] && ny <= r[3])
}

/// 2 つの矩形集合が異なるか。ドラッグ中の選択の再描画を、実際に選択が変わったとき
/// (ポインタがグリフ境界をまたいだとき) だけに絞り込むための比較述語。
/// 同じ入力を同じ計算にかければ同じ浮動小数点値が返る前提で、丸め誤差の吸収はしない
pub fn rects_changed(prev: &[[f64; 4]], next: &[[f64; 4]]) -> bool {
    prev != next
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
    fn bookmark_page_at_end_with_real_scroll_is_the_last_page() {
        let offs = page_offsets(&[100.0, 200.0, 50.0], 10.0);
        // クランプで最終ページの開始点 (320.0) に届かず 300.0 止まりでも、
        // 末尾までスクロールしていれば最終ページ扱いにする
        assert_eq!(bookmark_page(300.0, &offs, true), 2);
    }

    #[test]
    fn bookmark_page_at_end_but_scroll_zero_is_the_first_page() {
        let offs = page_offsets(&[100.0, 200.0, 50.0], 10.0);
        // ドキュメント全体がビューポートに収まっていて value も upper - page_size も
        // 0 のケース。at_end は理屈上 true になり得るが、正しい答えは先頭ページ
        assert_eq!(bookmark_page(0.0, &offs, true), 0);
    }

    #[test]
    fn bookmark_page_mid_scroll_matches_visible_page() {
        let offs = page_offsets(&[100.0, 200.0, 50.0], 10.0);
        assert_eq!(bookmark_page(150.0, &offs, false), 1);
        assert_eq!(bookmark_page(150.0, &offs, false), visible_page(150.0, &offs));
    }

    #[test]
    fn bookmark_page_at_start_is_the_first_page() {
        let offs = page_offsets(&[100.0, 200.0, 50.0], 10.0);
        assert_eq!(bookmark_page(0.0, &offs, false), 0);
    }

    #[test]
    fn bookmark_page_with_no_pages_is_zero() {
        // offsets が空でも offsets.len() - 1 で panic せず、先頭ページ扱いで返す
        assert_eq!(bookmark_page(10.0, &[], true), 0);
    }

    #[test]
    fn resize_queued_same_zoom_is_false() {
        assert!(!resize_queued(&[(500.0, 800.0), (500.0, 1000.0)], 1.2345, 1.2345));
    }

    #[test]
    fn resize_queued_tiny_delta_not_crossing_boundary_is_false() {
        // 800 * 1.0001 = 800.08 → 整数に丸めても 800 のまま
        assert!(!resize_queued(&[(500.0, 800.0)], 1.0, 1.0001));
    }

    #[test]
    fn resize_queued_one_page_crossing_boundary_is_true() {
        // 800 * 1.002 = 801.6 → 801 に変わる
        assert!(resize_queued(&[(500.0, 800.0)], 1.0, 1.002));
    }

    #[test]
    fn resize_queued_width_only_change_is_false() {
        // 幅は 1000 → 1005 (整数値が変わる) だが、高さは 100 → 100.5 (整数値は 100 のまま)。
        // ページは縦の Box に高さで並ぶので、vadjustment の changed を動かすのは高さの変化
        // だけ。幅を見てしまうとここが誤って true になり、`changed` が来ないのに待ち続けて
        // しまう (残件 1 と同じ不具合を、幅だけが変わるケースで再発させてしまう)
        assert!(!resize_queued(&[(1000.0, 100.0)], 1.0, 1.005));
    }

    #[test]
    fn resize_queued_only_one_of_several_pages_crossing_is_enough() {
        // 1 ページ目 (高さ 500) は境界をまたがず、2 ページ目 (高さ 1000) だけまたぐ
        assert!(resize_queued(&[(999.0, 500.0), (999.0, 1000.0)], 1.0, 1.0015));
    }

    #[test]
    fn resize_queued_empty_sizes_is_false() {
        assert!(!resize_queued(&[], 1.0, 2.0));
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

    #[test]
    fn rects_changed_when_length_differs() {
        let a = [[0.0, 0.0, 0.1, 0.1]];
        let b = [[0.0, 0.0, 0.1, 0.1], [0.2, 0.2, 0.3, 0.3]];
        assert!(rects_changed(&a, &b));
        assert!(rects_changed(&b, &a));
    }

    #[test]
    fn rects_unchanged_when_identical() {
        let a = [[0.0, 0.0, 0.1, 0.1], [0.2, 0.2, 0.3, 0.3]];
        let b = a;
        assert!(!rects_changed(&a, &b));
    }

    #[test]
    fn rects_changed_from_empty_to_nonempty() {
        assert!(rects_changed(&[], &[[0.0, 0.0, 0.1, 0.1]]));
    }

    #[test]
    fn rects_changed_from_nonempty_to_empty() {
        assert!(rects_changed(&[[0.0, 0.0, 0.1, 0.1]], &[]));
    }

    #[test]
    fn rects_changed_when_same_length_but_coordinates_differ() {
        // 1 行の中をポインタが横になぞる間、矩形の個数は 1 のまま座標だけ伸びる。
        // 長さだけ見る実装だとここを取りこぼす
        let a = [[0.0, 0.0, 0.1, 0.1]];
        let b = [[0.0, 0.0, 0.2, 0.1]];
        assert!(rects_changed(&a, &b));
    }
}
