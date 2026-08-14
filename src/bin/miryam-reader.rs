use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: miryam-reader <path/to.pdf>");
        return std::process::ExitCode::from(2);
    };
    let path = PathBuf::from(path);
    if !path.is_file() {
        eprintln!("miryam-reader: ファイルがありません: {}", path.display());
        return std::process::ExitCode::from(1);
    }
    let code = miryam::reader::ui::run(path);
    if code == gtk4::glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
