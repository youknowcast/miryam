use anyhow::Context;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

use crate::reader::store::Sidecar;

#[derive(Debug, Clone)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub name: String,
    pub highlight_count: usize,
    /// 空でないメモの数
    pub memo_count: usize,
    pub last_opened: Option<DateTime<Local>>,
    /// 0 始まり
    pub bookmark_page: usize,
    pub remarks: Vec<String>,
    /// サイドカーが読めなかった (壊れている)
    pub broken: bool,
}

/// ライブラリフォルダを走査して PDF の一覧を返す。
/// 並びは「最後に開いた新しい順 → 未読は名前順で末尾」
pub fn scan(dir: &Path, recursive: bool) -> anyhow::Result<Vec<LibraryEntry>> {
    let mut pdfs = Vec::new();
    collect(dir, recursive, &mut pdfs)
        .with_context(|| format!("{} が読めません", dir.display()))?;
    pdfs.sort();

    let mut entries: Vec<LibraryEntry> = pdfs.into_iter().map(entry_for).collect();
    entries.sort_by(|a, b| match (b.last_opened, a.last_opened) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
    .then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

fn collect(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for ent in std::fs::read_dir(dir)? {
        let ent = ent?;
        let path = ent.path();
        if path.is_dir() {
            if recursive && path.file_name().is_some_and(|n| n != ".miryam") {
                collect(&path, true, out)?;
            }
            continue;
        }
        let is_pdf = path
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"));
        if is_pdf {
            out.push(path);
        }
    }
    Ok(())
}

fn entry_for(path: PathBuf) -> LibraryEntry {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match Sidecar::load(&path) {
        Ok(Some(sc)) => LibraryEntry {
            highlight_count: sc.highlights.len(),
            memo_count: sc.highlights.iter().filter(|h| !h.memo.trim().is_empty()).count(),
            last_opened: Some(sc.opened_at),
            bookmark_page: sc.bookmark_page,
            remarks: sc.digest.map(|d| d.remarks).unwrap_or_default(),
            broken: false,
            path,
            name,
        },
        Ok(None) => LibraryEntry {
            highlight_count: 0,
            memo_count: 0,
            last_opened: None,
            bookmark_page: 0,
            remarks: Vec::new(),
            broken: false,
            path,
            name,
        },
        Err(e) => {
            eprintln!("miryam: {} の注釈が読めません: {e:#}", path.display());
            LibraryEntry {
                highlight_count: 0,
                memo_count: 0,
                last_opened: None,
                bookmark_page: 0,
                remarks: Vec::new(),
                broken: true,
                path,
                name,
            }
        }
    }
}

/// 本棚の 1 行分の表示文字列
pub fn format_entry(e: &LibraryEntry) -> String {
    if e.broken {
        return format!("{}  ⚠ 注釈が壊れています", e.name);
    }
    let Some(opened) = e.last_opened else {
        return format!("{}  未読", e.name);
    };
    format!(
        "{}  マーカー {} / メモ {}  p.{}  {}",
        e.name,
        e.highlight_count,
        e.memo_count,
        e.bookmark_page + 1,
        opened.format("%m/%d")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::Write;

    fn pdf(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("掘れること");
        }
        let mut f = std::fs::File::create(&path).expect("作成できること");
        f.write_all(b"%PDF-1.7\n").expect("書けること");
        path
    }

    #[test]
    fn scan_lists_pdfs_and_ignores_others() {
        let dir = tempfile::tempdir().expect("tempdir");
        pdf(dir.path(), "a.pdf");
        pdf(dir.path(), "b.PDF");
        std::fs::write(dir.path().join("c.txt"), "x").expect("書けること");
        std::fs::create_dir_all(dir.path().join("sub")).expect("掘れること");

        let entries = scan(dir.path(), false).expect("走査できること");
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a.pdf".to_string(), "b.PDF".to_string()]);
    }

    #[test]
    fn scan_recurses_only_when_asked() {
        let dir = tempfile::tempdir().expect("tempdir");
        pdf(dir.path(), "top.pdf");
        pdf(dir.path(), "sub/deep.pdf");

        assert_eq!(scan(dir.path(), false).expect("走査").len(), 1);
        assert_eq!(scan(dir.path(), true).expect("走査").len(), 2);
    }

    #[test]
    fn scan_skips_the_dot_miryam_dir_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        pdf(dir.path(), "a.pdf");
        pdf(dir.path(), ".miryam/decoy.pdf");
        let entries = scan(dir.path(), true).expect("走査");
        assert_eq!(entries.len(), 1, ".miryam の中は見ない");
    }

    #[test]
    fn entry_counts_come_from_the_sidecar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = pdf(dir.path(), "a.pdf");
        let mut sc = crate::reader::store::Sidecar::new(&p).expect("作れること");
        sc.bookmark_page = 7;
        for (i, memo) in ["", "書いた", "これも"].iter().enumerate() {
            sc.highlights.push(crate::reader::store::Highlight {
                id: format!("{i}"),
                page: i,
                color: "yellow".into(),
                rects: vec![[0.0, 0.0, 1.0, 0.1]],
                quote: "q".into(),
                memo: memo.to_string(),
                tags: vec![],
                llm: vec![],
                created_at: chrono::Local::now(),
            });
        }
        sc.digest = Some(crate::reader::store::DigestData {
            body: "抄訳".into(),
            remarks: vec!["面白かったです".into()],
            made_at: chrono::Local::now(),
            note_id: None,
        });
        sc.save(&p).expect("保存できること");

        let entries = scan(dir.path(), false).expect("走査");
        let e = &entries[0];
        assert_eq!(e.highlight_count, 3);
        assert_eq!(e.memo_count, 2, "空メモは数えない");
        assert_eq!(e.bookmark_page, 7);
        assert_eq!(e.remarks, vec!["面白かったです".to_string()]);
        assert!(e.last_opened.is_some());
        assert!(!e.broken);
    }

    #[test]
    fn entry_without_sidecar_is_zeroed() {
        let dir = tempfile::tempdir().expect("tempdir");
        pdf(dir.path(), "a.pdf");
        let e = &scan(dir.path(), false).expect("走査")[0];
        assert_eq!(e.highlight_count, 0);
        assert_eq!(e.memo_count, 0);
        assert_eq!(e.bookmark_page, 0);
        assert!(e.last_opened.is_none());
        assert!(!e.broken);
    }

    #[test]
    fn broken_sidecar_is_flagged_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        pdf(dir.path(), "a.pdf");
        std::fs::create_dir_all(dir.path().join(".miryam")).expect("掘れること");
        std::fs::write(dir.path().join(".miryam/a.pdf.json"), "{ broken").expect("書けること");

        let entries = scan(dir.path(), false).expect("走査は成功する");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].broken, "壊れている印が付く");
    }

    #[test]
    fn missing_dir_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(scan(&dir.path().join("nope"), false).is_err());
    }

    #[test]
    fn entries_are_sorted_by_last_opened_then_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let old = pdf(dir.path(), "old.pdf");
        let new = pdf(dir.path(), "new.pdf");
        pdf(dir.path(), "zzz.pdf");

        let mut a = crate::reader::store::Sidecar::new(&old).expect("作れること");
        a.opened_at = chrono::Local::now() - chrono::Duration::days(2);
        a.save(&old).expect("保存");
        let mut b = crate::reader::store::Sidecar::new(&new).expect("作れること");
        b.opened_at = chrono::Local::now();
        b.save(&new).expect("保存");

        let names: Vec<_> = scan(dir.path(), false)
            .expect("走査")
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["new.pdf".to_string(), "old.pdf".to_string(), "zzz.pdf".to_string()],
            "既読が新しい順、未読は名前順で末尾"
        );
    }

    #[test]
    fn format_entry_shows_counts_bookmark_and_last_opened() {
        let opened = chrono::Local
            .with_ymd_and_hms(2026, 8, 3, 21, 5, 0)
            .single()
            .expect("実在する時刻");
        let e = LibraryEntry {
            path: "/x/a.pdf".into(),
            name: "a.pdf".into(),
            highlight_count: 3,
            memo_count: 2,
            bookmark_page: 11,
            last_opened: Some(opened),
            remarks: vec![],
            broken: false,
        };
        let s = format_entry(&e);
        assert!(s.contains("a.pdf"));
        assert!(s.contains("マーカー 3"), "マーカー件数が出る: {s}");
        assert!(s.contains("メモ 2"), "メモ件数が出る: {s}");
        assert!(s.contains("p.12"), "しおりは 1 始まりで出す: {s}");
        assert!(s.contains("08/03"), "最終閲覧が出る: {s}");
    }

    #[test]
    fn format_entry_marks_unread_and_broken() {
        let base = LibraryEntry {
            path: "/x/a.pdf".into(),
            name: "a.pdf".into(),
            highlight_count: 0,
            memo_count: 0,
            bookmark_page: 0,
            last_opened: None,
            remarks: vec![],
            broken: false,
        };
        assert!(format_entry(&base).contains("未読"));
        let broken = LibraryEntry { broken: true, ..base };
        assert!(format_entry(&broken).contains("注釈が壊れています"));
    }
}
