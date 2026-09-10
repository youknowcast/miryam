use std::path::Path;
use std::sync::mpsc;

use cairo::{ImageSurface, ImageSurfaceDataOwned};
use gtk4::glib;

/// レンダー依頼。`generation` は依頼時点のズーム世代。ズームが変わったあとに届いた
/// 古い世代の結果は受け取り側で捨てられる
pub struct RenderJob {
    pub page: usize,
    pub zoom: f64,
    pub generation: u64,
}

/// レンダー結果。`ImageSurfaceDataOwned` (Send) で送り、受け取り側で
/// `into_inner()` により `ImageSurface` に戻す
pub struct RenderDone {
    pub page: usize,
    pub zoom: f64,
    pub generation: u64,
    pub data: ImageSurfaceDataOwned,
}

/// ページを 1 回だけレンダーして surface に残す。`draw_func` はこれを blit するだけにし、
/// スクロールや再描画のたびに poppler のレンダー (画像が重い PDF では 1 ページ数百 ms)
/// をやり直さないための単位
pub fn render_page(page: &poppler::Page, zoom: f64) -> ImageSurface {
    let (w, h) = page.size();
    let surface = ImageSurface::create(
        cairo::Format::Rgb24,
        (w * zoom).ceil() as i32,
        (h * zoom).ceil() as i32,
    )
    .expect("surface を作れること");
    let cr = cairo::Context::new(&surface).expect("context を作れること");
    // poppler の render は透明な背景に描くので、まず紙の白で塗る
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.paint();
    cr.scale(zoom, zoom);
    page.render(&cr);
    surface
}

/// ワーカースレッドを起動して (依頼, 結果受け取り) を返す。
/// 依頼側の `Sender` が全て落ちるとスレッドは終了する。
/// スレッドは自分で `Document` を開く (poppler のページは Send でないため、
/// メインスレッドの document を共有せず、開き直しは 3ms 程度で無視できる)
pub fn start(pdf_path: &Path) -> (mpsc::Sender<RenderJob>, mpsc::Receiver<RenderDone>) {
    let (job_tx, job_rx) = mpsc::channel::<RenderJob>();
    let (done_tx, done_rx) = mpsc::channel::<RenderDone>();
    let uri = glib::filename_to_uri(pdf_path, None)
        .map_err(|e| format!("パスを URI にできません: {e}"))
        .expect("reader を開ける段階で既に変換済みのパス");
    std::thread::spawn(move || {
        let Ok(doc) = poppler::Document::from_file(&uri, None) else {
            return;
        };
        for job in job_rx {
            let Some(page) = doc.page(job.page as i32) else {
                continue;
            };
            let surface = render_page(&page, job.zoom);
            let Ok(data) = surface.take_data() else {
                continue;
            };
            let done = RenderDone {
                page: job.page,
                zoom: job.zoom,
                generation: job.generation,
                data,
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    fn fixture_pdf(dir: &Path, name: &str) -> std::path::PathBuf {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>".to_string(),
        ];
        let mut buf = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(buf.len());
            buf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref_offset = buf.len();
        buf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        buf.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        buf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
                objects.len() + 1
            )
            .as_bytes(),
        );
        let path = dir.join(name);
        std::fs::File::create(&path)
            .expect("作成できること")
            .write_all(&buf)
            .expect("書けること");
        path
    }

    #[test]
    fn worker_renders_a_job_and_returns_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_pdf(dir.path(), "fixture.pdf");
        let (tx, rx) = start(&path);

        tx.send(RenderJob {
            page: 0,
            zoom: 2.0,
            generation: 7,
        })
        .expect("依頼を送れること");
        let done = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("結果が返ること");
        assert_eq!(done.page, 0, "ページ番号がそのまま返る");
        assert_eq!(done.generation, 7, "世代がそのまま返る");
        let surface = done.data.into_inner();
        assert_eq!(
            (surface.width(), surface.height()),
            (600, 400),
            "300x200 のページを 2 倍で"
        );
    }

    #[test]
    fn render_page_scales_the_surface_to_the_zoom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_pdf(dir.path(), "fixture.pdf");
        let doc = poppler::Document::from_file(&format!("file://{}", path.display()), None)
            .expect("最小 PDF を開けること");
        let page = doc.page(0).expect("1 ページ目");

        let s = render_page(&page, 2.0);
        assert_eq!((s.width(), s.height()), (600, 400));
    }

    #[test]
    fn render_page_paints_white_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_pdf(dir.path(), "fixture.pdf");
        let doc = poppler::Document::from_file(&format!("file://{}", path.display()), None)
            .expect("最小 PDF を開けること");
        let page = doc.page(0).expect("1 ページ目");

        let mut s = render_page(&page, 1.0);
        let data = s.data().expect("ピクセルを読めること");
        // Rgb24 はリトルエンディアンで B,G,R の順。1 ピクセル目 (0,0) が白なら全部 255
        assert_eq!(&data[0..3], &[255, 255, 255], "紙の白で塗られていること");
    }
}
