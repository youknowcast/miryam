pub mod pages;

use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use std::path::PathBuf;
use std::rc::Rc;

use crate::reader::geom;

/// ページ間の隙間 (px)
const PAGE_GAP: f64 = 12.0;

pub fn run(path: PathBuf) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.youknow.miryam.reader")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let path = Rc::new(path);
    app.connect_activate(move |app| {
        if let Err(e) = build_window(app, &path) {
            eprintln!("miryam-reader: {e:#}");
            show_fatal(app, &format!("{e:#}"));
        }
    });
    app.run_with_args::<&str>(&[])
}

fn build_window(app: &gtk::Application, path: &PathBuf) -> anyhow::Result<()> {
    let uri = glib::filename_to_uri(path, None)
        .map_err(|e| anyhow::anyhow!("パスを URI にできません: {e}"))?;
    let doc = poppler::Document::from_file(&uri, None)
        .map_err(|e| anyhow::anyhow!("PDF が開けません: {e}"))?;
    if doc.n_pages() == 0 {
        anyhow::bail!("ページがありません");
    }

    let view = Rc::new(pages::PageView::new(&doc, PAGE_GAP)?);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(view.widget()));

    let page_label = gtk::Label::new(Some(&format!("p.1/{}", doc.n_pages())));
    let zoom_label = gtk::Label::new(Some("100%"));

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(6);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);

    let zoom_out = gtk::Button::with_label("−");
    let zoom_in = gtk::Button::with_label("＋");
    let fit = gtk::Button::with_label("幅合わせ");
    toolbar.append(&page_label);
    toolbar.append(&zoom_out);
    toolbar.append(&zoom_label);
    toolbar.append(&zoom_in);
    toolbar.append(&fit);

    let apply_zoom = {
        let view = view.clone();
        let zoom_label = zoom_label.clone();
        Rc::new(move |z: f64| {
            view.set_zoom(z);
            zoom_label.set_text(&format!("{:.0}%", view.zoom() * 100.0));
        })
    };

    {
        let apply = apply_zoom.clone();
        let view = view.clone();
        zoom_in.connect_clicked(move |_| apply(view.zoom() * 1.25));
    }
    {
        let apply = apply_zoom.clone();
        let view = view.clone();
        zoom_out.connect_clicked(move |_| apply(view.zoom() / 1.25));
    }
    {
        let apply = apply_zoom.clone();
        let view = view.clone();
        let scrolled = scrolled.clone();
        fit.connect_clicked(move |_| {
            let (w, _) = view.page_sizes()[0];
            apply(geom::fit_width_scale(w, scrolled.width() as f64));
        });
    }

    // スクロールに合わせてページ番号を更新する
    {
        let view = view.clone();
        let page_label = page_label.clone();
        let total = doc.n_pages();
        let vadj = scrolled.vadjustment();
        vadj.connect_value_changed(move |adj| {
            let offsets = geom::page_offsets(&view.scaled_heights(), PAGE_GAP);
            // container の set_margin_top(MARGIN) 分、ページ 0 の上端はスクロール座標 MARGIN にある
            let n = geom::visible_page(adj.value() - geom::MARGIN, &offsets);
            page_label.set_text(&format!("p.{}/{}", n + 1, total));
        });
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&toolbar);
    root.append(&scrolled);

    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "miryam-reader".into());
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .default_width(1200)
        .default_height(800)
        .child(&root)
        .build();
    window.present();

    // 開いた直後は幅合わせにする
    let apply = apply_zoom.clone();
    let view2 = view.clone();
    let scrolled2 = scrolled.clone();
    glib::idle_add_local_once(move || {
        let (w, _) = view2.page_sizes()[0];
        apply(geom::fit_width_scale(w, scrolled2.width() as f64));
    });

    Ok(())
}

fn show_fatal(app: &gtk::Application, msg: &str) {
    let dialog = gtk::MessageDialog::builder()
        .application(app)
        .message_type(gtk::MessageType::Error)
        .buttons(gtk::ButtonsType::Close)
        .text("PDF を開けませんでした")
        .secondary_text(msg)
        .build();
    let app = app.clone();
    dialog.connect_response(move |d, _| {
        d.close();
        app.quit();
    });
    dialog.present();
}
