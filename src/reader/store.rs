use anyhow::Context;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// サイドカー JSON のスキーマ版。読めるのはこの版だけ
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct PdfMeta {
    pub name: String,
    /// 「名前は違うが中身は同じ」の検出にだけ使う
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LlmQa {
    /// どの操作の結果か (`ask::Action::kind`)。
    /// 古いサイドカーには無いので既定は空文字
    #[serde(default)]
    pub kind: String,
    pub q: String,
    pub a: String,
    pub at: DateTime<Local>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Highlight {
    pub id: String,
    /// 0 始まりのページ番号
    pub page: usize,
    pub color: String,
    /// ページサイズに対する正規化座標 [x0, y0, x1, y1] (左上原点)
    pub rects: Vec<[f64; 4]>,
    pub quote: String,
    #[serde(default)]
    pub memo: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub llm: Vec<LlmQa>,
    pub created_at: DateTime<Local>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DigestData {
    pub body: String,
    #[serde(default)]
    pub remarks: Vec<String>,
    pub made_at: DateTime<Local>,
    #[serde(default)]
    pub note_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sidecar {
    pub schema: u32,
    pub pdf: PdfMeta,
    #[serde(default)]
    pub bookmark_page: usize,
    pub opened_at: DateTime<Local>,
    #[serde(default)]
    pub highlights: Vec<Highlight>,
    #[serde(default)]
    pub digest: Option<DigestData>,
}

/// `<PDF のあるフォルダ>/.miryam/<PDF 名>.json`
pub fn sidecar_path(pdf: &Path) -> PathBuf {
    let parent = pdf.parent().unwrap_or_else(|| Path::new("."));
    let name = pdf.file_name().unwrap_or_default().to_string_lossy();
    parent.join(".miryam").join(format!("{name}.json"))
}

impl Sidecar {
    pub fn new(pdf: &Path) -> anyhow::Result<Self> {
        let size = std::fs::metadata(pdf)
            .with_context(|| format!("PDF が読めません: {}", pdf.display()))?
            .len();
        Ok(Self {
            schema: SCHEMA_VERSION,
            pdf: PdfMeta {
                name: pdf.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                size,
            },
            bookmark_page: 0,
            opened_at: Local::now(),
            highlights: Vec::new(),
            digest: None,
        })
    }

    /// 無ければ `Ok(None)`。壊れていれば `Err` (黙って作り直さない)
    pub fn load(pdf: &Path) -> anyhow::Result<Option<Self>> {
        let path = sidecar_path(pdf);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("{} が読めません", path.display())),
        };
        let sc: Self = serde_json::from_str(&text)
            .with_context(|| format!("{} の JSON が壊れています", path.display()))?;
        if sc.schema != SCHEMA_VERSION {
            anyhow::bail!(
                "{} は未知のスキーマ版です (schema={}, 対応は {})",
                path.display(),
                sc.schema,
                SCHEMA_VERSION
            );
        }
        Ok(Some(sc))
    }

    /// 一時ファイル + rename の原子的書き込み。途中で失敗したら `.tmp` を残さない
    pub fn save(&self, pdf: &Path) -> anyhow::Result<()> {
        let path = sidecar_path(pdf);
        let dir = path.parent().expect("sidecar_path は必ず親を持つ");
        std::fs::create_dir_all(dir)
            .with_context(|| format!("{} が作れません", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).context("JSON への変換に失敗しました")?;
        if let Err(e) =
            std::fs::write(&tmp, text).with_context(|| format!("{} が書けません", tmp.display()))
        {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, &path)
            .with_context(|| format!("{} への差し替えに失敗しました", path.display()))
        {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    }
}

/// 時系列に並ぶ ID。`<ミリ秒 epoch の 13 桁ゼロ埋め>-<seq の 4 桁ゼロ埋め>`
pub fn new_id(now: DateTime<Local>, seq: u32) -> String {
    format!("{:013}-{:04}", now.timestamp_millis().max(0), seq % 10000)
}

/// ページ実寸の矩形を 0.0〜1.0 の正規化座標にする。ページ外へはみ出した分はクランプする
pub fn normalize_rect(x0: f64, y0: f64, x1: f64, y1: f64, page_w: f64, page_h: f64) -> [f64; 4] {
    if page_w <= 0.0 || page_h <= 0.0 {
        return [0.0; 4];
    }
    let clamp = |v: f64| v.clamp(0.0, 1.0);
    [
        clamp(x0.min(x1) / page_w),
        clamp(y0.min(y1) / page_h),
        clamp(x0.max(x1) / page_w),
        clamp(y0.max(y1) / page_h),
    ]
}

/// 正規化座標をページ実寸に戻す
pub fn denormalize_rect(r: &[f64; 4], page_w: f64, page_h: f64) -> (f64, f64, f64, f64) {
    (r[0] * page_w, r[1] * page_h, r[2] * page_w, r[3] * page_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// tempdir に空でない PDF もどきを作り、そのパスを返す
    fn fixture(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("作成できること");
        f.write_all(b"%PDF-1.7\n").expect("書けること");
        path
    }

    #[test]
    fn sidecar_path_is_dot_miryam_sibling() {
        let p = sidecar_path(std::path::Path::new("/a/b/foo.pdf"));
        assert_eq!(p, std::path::PathBuf::from("/a/b/.miryam/foo.pdf.json"));
    }

    #[test]
    fn load_returns_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        assert!(Sidecar::load(&pdf).expect("読めること").is_none());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");

        let mut sc = Sidecar::new(&pdf).expect("作れること");
        sc.bookmark_page = 12;
        sc.highlights.push(Highlight {
            id: "01".into(),
            page: 12,
            color: "yellow".into(),
            rects: vec![[0.12, 0.33, 0.88, 0.36]],
            quote: "引用文".into(),
            memo: "メモ".into(),
            tags: vec!["重要".into()],
            llm: vec![],
            created_at: chrono::Local::now(),
        });
        sc.save(&pdf).expect("保存できること");

        let loaded = Sidecar::load(&pdf).expect("読めること").expect("存在する");
        assert_eq!(loaded.schema, SCHEMA_VERSION);
        assert_eq!(loaded.pdf.name, "foo.pdf");
        assert_eq!(loaded.pdf.size, 9);
        assert_eq!(loaded.bookmark_page, 12);
        assert_eq!(loaded.highlights.len(), 1);
        assert_eq!(loaded.highlights[0].quote, "引用文");
        assert_eq!(loaded.highlights[0].rects[0][2], 0.88);
        assert_eq!(loaded.highlights[0].tags, vec!["重要".to_string()]);
    }

    #[test]
    fn save_creates_dot_miryam_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        Sidecar::new(&pdf).expect("作れること").save(&pdf).expect("保存できること");
        assert!(dir.path().join(".miryam/foo.pdf.json").is_file());
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        Sidecar::new(&pdf).expect("作れること").save(&pdf).expect("保存できること");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(".miryam"))
            .expect("読めること")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "foo.pdf.json")
            .collect();
        assert!(leftovers.is_empty(), "一時ファイルが残っている: {leftovers:?}");
    }

    #[test]
    fn save_failure_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        // 差し替え先をディレクトリにしておくと rename が失敗する
        std::fs::create_dir_all(dir.path().join(".miryam/foo.pdf.json")).expect("掘れること");

        let sc = Sidecar::new(&pdf).expect("作れること");
        assert!(sc.save(&pdf).is_err(), "rename 失敗はエラーになる");

        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(".miryam"))
            .expect("読めること")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "foo.pdf.json")
            .collect();
        assert!(leftovers.is_empty(), "一時ファイルが残っている: {leftovers:?}");
    }

    #[test]
    fn broken_json_is_an_error_not_a_silent_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        std::fs::create_dir_all(dir.path().join(".miryam")).expect("掘れること");
        std::fs::write(dir.path().join(".miryam/foo.pdf.json"), "{ broken").expect("書けること");
        assert!(Sidecar::load(&pdf).is_err(), "壊れた JSON はエラー");
    }

    #[test]
    fn future_schema_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        std::fs::create_dir_all(dir.path().join(".miryam")).expect("掘れること");
        std::fs::write(
            dir.path().join(".miryam/foo.pdf.json"),
            r#"{"schema":99,"pdf":{"name":"foo.pdf","size":9},"bookmark_page":0,
                "opened_at":"2026-08-13T10:00:00+09:00","highlights":[]}"#,
        )
        .expect("書けること");
        let err = Sidecar::load(&pdf).expect_err("未知の schema は拒否");
        assert!(err.to_string().contains("99"));
    }

    #[test]
    fn a_sidecar_written_before_kind_existed_still_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        std::fs::create_dir_all(dir.path().join(".miryam")).expect("掘れること");
        // kind が無い時代のサイドカー。既存ファイルが読めなくなってはいけない
        std::fs::write(
            dir.path().join(".miryam/foo.pdf.json"),
            r#"{"schema":1,"pdf":{"name":"foo.pdf","size":9},"bookmark_page":0,
                "opened_at":"2026-08-13T10:00:00+09:00",
                "highlights":[{"id":"1","page":0,"color":"yellow",
                  "rects":[[0.0,0.0,0.1,0.1]],"quote":"q","memo":"",
                  "tags":[],"llm":[{"q":"なに?","a":"これ","at":"2026-08-13T10:00:00+09:00"}],
                  "created_at":"2026-08-13T10:00:00+09:00"}]}"#,
        )
        .expect("書けること");

        let sc = Sidecar::load(&pdf).expect("読めること").expect("存在する");
        assert_eq!(sc.highlights[0].llm.len(), 1);
        assert_eq!(sc.highlights[0].llm[0].kind, "", "kind が無ければ空文字");
    }

    #[test]
    fn kind_roundtrips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = fixture(dir.path(), "foo.pdf");
        let mut sc = Sidecar::new(&pdf).expect("作れること");
        sc.highlights.push(Highlight {
            id: "1".into(),
            page: 0,
            color: "yellow".into(),
            rects: vec![[0.0, 0.0, 0.1, 0.1]],
            quote: "q".into(),
            memo: String::new(),
            tags: vec![],
            llm: vec![LlmQa {
                kind: "ask".into(),
                q: "なに?".into(),
                a: "これ".into(),
                at: chrono::Local::now(),
            }],
            created_at: chrono::Local::now(),
        });
        sc.save(&pdf).expect("保存できること");
        let loaded = Sidecar::load(&pdf).expect("読めること").expect("存在する");
        assert_eq!(loaded.highlights[0].llm[0].kind, "ask");
    }

    #[test]
    fn new_id_is_sortable_and_unique() {
        let t0 = chrono::Local::now();
        let t1 = t0 + chrono::Duration::milliseconds(1);
        assert!(new_id(t0, 0) < new_id(t1, 0), "時刻順に並ぶ");
        assert_ne!(new_id(t0, 0), new_id(t0, 1), "同時刻でも seq で分かれる");
    }

    #[test]
    fn rect_normalization_roundtrips() {
        let n = normalize_rect(60.0, 100.0, 240.0, 130.0, 600.0, 800.0);
        assert!((n[0] - 0.1).abs() < 1e-9);
        assert!((n[1] - 0.125).abs() < 1e-9);
        assert!((n[2] - 0.4).abs() < 1e-9);
        let (x0, y0, x1, y1) = denormalize_rect(&n, 600.0, 800.0);
        assert!((x0 - 60.0).abs() < 1e-9);
        assert!((y0 - 100.0).abs() < 1e-9);
        assert!((x1 - 240.0).abs() < 1e-9);
        assert!((y1 - 130.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_rect_clamps_out_of_page_values() {
        let n = normalize_rect(-10.0, -5.0, 700.0, 900.0, 600.0, 800.0);
        assert_eq!(n, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn normalize_rect_with_zero_page_size_is_empty() {
        assert_eq!(normalize_rect(1.0, 2.0, 3.0, 4.0, 0.0, 800.0), [0.0; 4]);
    }
}
