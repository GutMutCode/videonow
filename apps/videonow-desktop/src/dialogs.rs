//! The shell's own file dialogs (open and save a project), shown without blocking the window: rfd
//! runs on a helper thread and the answer arrives on a channel the shell polls each frame.

use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc;

/// What a pending dialog is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Choose a `.videonow` project folder to open.
    OpenProject,
    /// Choose where to save the project (a new `.videonow` folder).
    SaveProject,
}

pub struct Pending {
    pub purpose: Purpose,
    rx: mpsc::Receiver<Option<PathBuf>>,
}

impl Pending {
    /// `Some(answer)` once the dialog closed (`answer` is `None` when it was cancelled).
    pub fn poll(&self) -> Option<Option<PathBuf>> {
        match self.rx.try_recv() {
            Ok(a) => Some(a),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }
}

/// Runs `future` to completion on this thread.
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// Show a dialog for `purpose`; `start` is the folder it opens in.
pub fn show(purpose: Purpose, start: Option<PathBuf>, ctx: &egui::Context) -> Pending {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    let mut d = rfd::AsyncFileDialog::new();
    if let Some(dir) = start.filter(|d| d.is_dir()) {
        d = d.set_directory(dir);
    }
    let fut: std::pin::Pin<Box<dyn Future<Output = Option<PathBuf>> + Send>> = match purpose {
        // a project is a folder
        Purpose::OpenProject => {
            let f = d.set_title("Open VideoNow Project (a .videonow folder)").pick_folder();
            Box::pin(async move { f.await.map(|h| h.path().to_path_buf()) })
        }
        Purpose::SaveProject => {
            let f = d.set_title("Save VideoNow Project").set_file_name("Untitled.videonow").save_file();
            Box::pin(async move { f.await.map(|h| h.path().to_path_buf()) })
        }
    };
    let spawned = std::thread::Builder::new().name("project dialog".into()).spawn(move || {
        let _ = tx.send(block_on(fut));
        ctx.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("couldn't show the dialog: {e}");
    }
    Pending { purpose, rx }
}
