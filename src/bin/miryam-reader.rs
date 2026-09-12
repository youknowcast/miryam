use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let mut present = false;
    let mut path: Option<PathBuf> = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--present" {
            present = true;
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            eprintln!("usage: miryam-reader [--present] <path/to.pdf>");
            return std::process::ExitCode::from(2);
        }
    }
    let Some(path) = path else {
        eprintln!("usage: miryam-reader [--present] <path/to.pdf>");
        return std::process::ExitCode::from(2);
    };
    if !path.is_file() {
        eprintln!("miryam-reader: ファイルがありません: {}", path.display());
        return std::process::ExitCode::from(1);
    }
    let code = if present {
        miryam::reader::present::run(path)
    } else {
        miryam::reader::ui::run(path)
    };
    if code == gtk4::glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
