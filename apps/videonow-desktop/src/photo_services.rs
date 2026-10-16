//! What the image editor needs from the platform: decoders and encoders, file writes, file
//! dialogs. A smaller set than PhotoCraft's own application wires (no crash recovery of its own:
//! the project is saved as a whole).

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use photocraft_doc::Document;
use photocraft_ui_egui::{ExportSettings, FileDialogAnswer, FileDialogReply, FileDialogRequest, Services};

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    photocraft_format::atomic_write(path, bytes).map_err(|e| e.to_string())
}

fn save_file(doc: &Document, path: &str, settings: &ExportSettings, ctl: &photocraft_ui_egui::jobs_ui::SaveCtl) -> Result<Vec<String>, String> {
    ctl.progress(0.0, "Encoding");
    let opts = settings.io_options();
    let r = photocraft_io::export(doc, path, &opts).map_err(|e| e.to_string())?;
    ctl.commit()?;
    ctl.progress(0.9, "Writing");
    write_atomic(Path::new(path), &r.bytes)?;
    ctl.progress(1.0, "");
    Ok(r.warnings)
}

const OPEN_EXTENSIONS: &[&str] = &["pcraft", "psd", "psb", "png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif", "tga", "exr", "hdr", "svg", "dng", "heic", "avif", "jxl"];

/// rfd's dialog on a helper thread; the answer goes back through `reply` (the editor polls it).
fn show_file_dialog(request: FileDialogRequest, _parent: Option<&eframe::Frame>, reply: FileDialogReply) {
    let mut dialog = rfd::AsyncFileDialog::new();
    let answer: Pin<Box<dyn Future<Output = Option<FileDialogAnswer>> + Send>> = match request {
        FileDialogRequest::Open { multiple, initial_dir, extensions } => {
            if let Some(dir) = initial_dir {
                dialog = dialog.set_directory(dir);
            }
            let exts: Vec<String> = extensions.unwrap_or_else(|| OPEN_EXTENSIONS.iter().map(|s| s.to_string()).collect());
            let refs: Vec<&str> = exts.iter().map(String::as_str).collect();
            dialog = dialog.add_filter("Images", &refs);
            if multiple {
                let picked = dialog.pick_files();
                Box::pin(async move { picked.await.map(|files| FileDialogAnswer::Paths(files.iter().map(|f| f.path().to_string_lossy().into_owned()).collect())) })
            } else {
                let picked = dialog.pick_file();
                Box::pin(async move { picked.await.map(|f| FileDialogAnswer::Paths(vec![f.path().to_string_lossy().into_owned()])) })
            }
        }
        FileDialogRequest::Folder { initial_dir } => {
            if let Some(dir) = initial_dir {
                dialog = dialog.set_directory(dir);
            }
            let picked = dialog.pick_folder();
            Box::pin(async move { picked.await.map(|f| FileDialogAnswer::FolderTo(f.path().to_string_lossy().into_owned())) })
        }
        FileDialogRequest::Save { suggested } => {
            let p = Path::new(&suggested);
            if let Some(name) = p.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
            if let Some(dir) = p.parent().filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(dir);
            }
            let picked = dialog.save_file();
            Box::pin(async move { picked.await.map(|f| FileDialogAnswer::SaveTo(f.path().to_string_lossy().into_owned())) })
        }
    };
    if let Err(e) = std::thread::Builder::new().name("file dialog".into()).spawn(move || reply.send(crate::dialogs::block_on(answer))) {
        log::error!("couldn't wait for the file dialog: {e}");
    }
}

/// The services the image editor runs with. `dialogs: false` leaves file dialogs out (offscreen
/// runs and tests, where nobody could answer one).
pub fn services(dialogs: bool) -> Services {
    Services {
        import: Some(Box::new(|name: &str, bytes: &[u8], depth: usize| {
            photocraft_io::import_with_svg_group_depth(name, bytes, depth).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
        })),
        export: Some(Box::new(|doc: &Document, path: &str, settings: &ExportSettings| {
            photocraft_io::export(doc, path, &settings.io_options()).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
        })),
        write: Some(Box::new(|path: &str, bytes: &[u8]| write_atomic(Path::new(path), bytes))),
        save_file: Some(Arc::new(save_file)),
        file_dialog: if dialogs { Some(Box::new(show_file_dialog)) } else { None },
        open_url: Some(Box::new(|url: &str| open::that(url).map_err(|e| e.to_string()))),
        ..Default::default()
    }
}
