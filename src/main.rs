mod phrases;
mod scheduler;
mod ui;

use gtk4 as gtk;
use gtk::{glib, prelude::*};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.youknow.miryam")
        .build();
    app.connect_activate(|app| {
        if let Err(err) = activate(app) {
            eprintln!("miryam: 起動に失敗しました: {err:#}");
            std::process::exit(1);
        }
    });
    app.run()
}

fn activate(app: &gtk::Application) -> anyhow::Result<()> {
    let ui = ui::build(app)?;
    ui.show_bubble("動作確認用の吹き出しです");
    Ok(())
}
