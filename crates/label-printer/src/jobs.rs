//! The render-and-print worker.
//!
//! One background thread owns pdfium and a cache of the open file. The UI
//! sends [`JobRequest`]s and polls [`JobEvent`]s; slow work (decoding, PDF
//! rendering, dithering, spooler and USB writes) never runs on the UI thread.
//! When several previews queue up (dragging a size field), only the newest is
//! rendered.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;

use image::GrayImage;
use tspl_core::{encode, parse_target, Config, Frame};

use crate::content::{self, LabelPx, PrepareOptions};
use crate::pdf::{self, PdfEngine};

/// Message for printing or previewing with nothing open.
pub const NOTHING_OPEN: &str = "Open a PDF or image first";

/// File extensions decoded as images.
pub const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "bmp", "webp", "gif", "tif", "tiff"];

/// What to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Nothing chosen yet: the window shows a drop zone.
    Empty,
    /// The built-in test card.
    TestCard,
    /// An image or PDF on disk.
    File(PathBuf),
}

/// How content is laid out on the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Label size in dots.
    pub label: LabelPx,
    /// Crop, rotation, halftone, invert.
    pub options: PrepareOptions,
}

/// Which pages to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pages {
    /// One page, zero-based.
    One(usize),
    /// Every page of a PDF.
    All,
}

/// Where a job goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// A `tspl-core` target spec: `usb:…`, `raw:…`, `serial:…`.
    Wired(String),
    /// The Bluetooth LE worker; the encoded job is handed back to the UI.
    Bluetooth,
}

/// Work for the worker.
#[derive(Debug)]
pub enum JobRequest {
    /// Render the preview of one page.
    Preview {
        /// Echoed back so stale results are ignored.
        generation: u64,
        /// Content.
        source: Source,
        /// Zero-based page.
        page: usize,
        /// Label and options.
        layout: Layout,
    },
    /// Render, encode and send.
    Print {
        /// Content.
        source: Source,
        /// Pages to print.
        pages: Pages,
        /// Label and options.
        layout: Layout,
        /// Printer settings.
        config: Config,
        /// Target.
        destination: Destination,
    },
    /// Send prepared bytes (feed, calibrate).
    Raw {
        /// Short name for messages.
        what: &'static str,
        /// Job bytes.
        bytes: Vec<u8>,
        /// Target.
        destination: Destination,
    },
}

/// Results.
#[derive(Debug)]
pub enum JobEvent {
    /// A preview is ready.
    Preview {
        /// From the request.
        generation: u64,
        /// The black-and-white label exactly as it prints.
        frame: Frame,
        /// Page count for PDFs.
        pages: Option<usize>,
    },
    /// The preview could not be built.
    PreviewFailed {
        /// From the request.
        generation: u64,
        /// User-facing reason.
        message: String,
    },
    /// Rendering for print started (`pages` pages).
    Rendering(usize),
    /// Bytes for the Bluetooth worker.
    ForBluetooth {
        /// What it is, for messages.
        what: String,
        /// Encoded job.
        bytes: Vec<u8>,
    },
    /// A wired print finished.
    Printed {
        /// What was printed, for messages.
        what: String,
        /// Bytes sent.
        bytes: usize,
        /// Where.
        target: String,
    },
    /// A print failed.
    Failed(String),
}

/// Handle to the worker.
pub struct Jobs {
    /// Requests in.
    pub requests: Sender<JobRequest>,
    /// Events out.
    pub events: Receiver<JobEvent>,
}

/// Starts the worker. `wake` is called after every event.
#[must_use]
pub fn spawn(wake: impl Fn() + Send + 'static) -> Jobs {
    let (request_tx, request_rx) = channel();
    let (event_tx, event_rx) = channel();

    // The thread ends when the UI drops the request sender.
    std::thread::Builder::new()
        .name("render".to_string())
        .spawn(move || {
            let send = |event: JobEvent| {
                let _ = event_tx.send(event);
                wake();
            };
            Worker::default().run(&request_rx, &send);
        })
        .expect("spawn render thread");

    Jobs {
        requests: request_tx,
        events: event_rx,
    }
}

/// A decoded file, kept between previews.
enum Loaded {
    Image(GrayImage),
    Pdf { bytes: Vec<u8>, pages: usize },
}

#[derive(Default)]
struct Worker {
    pdf: Option<Result<PdfEngine, String>>,
    cache: Option<(PathBuf, Option<SystemTime>, std::rc::Rc<Loaded>)>,
}

impl Worker {
    fn run(&mut self, requests: &Receiver<JobRequest>, send: &dyn Fn(JobEvent)) {
        let mut queue: VecDeque<JobRequest> = VecDeque::new();
        loop {
            if queue.is_empty() {
                match requests.recv() {
                    Ok(request) => queue.push_back(request),
                    Err(_) => return,
                }
            }
            // Collapse queued previews to the newest one.
            while let Ok(request) = requests.try_recv() {
                if matches!(request, JobRequest::Preview { .. }) {
                    queue.retain(|queued| !matches!(queued, JobRequest::Preview { .. }));
                }
                queue.push_back(request);
            }
            let Some(request) = queue.pop_front() else {
                continue;
            };
            self.handle(request, send);
        }
    }

    fn handle(&mut self, request: JobRequest, send: &dyn Fn(JobEvent)) {
        match request {
            JobRequest::Preview {
                generation,
                source,
                page,
                layout,
            } => match self.frame(&source, page, layout) {
                Ok((frame, pages)) => send(JobEvent::Preview {
                    generation,
                    frame,
                    pages,
                }),
                Err(message) => send(JobEvent::PreviewFailed {
                    generation,
                    message,
                }),
            },
            JobRequest::Print {
                source,
                pages,
                layout,
                config,
                destination,
            } => match self.encode_pages(&source, pages, layout, &config, send) {
                Ok((what, bytes)) => deliver(what, bytes, &destination, send),
                Err(message) => send(JobEvent::Failed(message)),
            },
            JobRequest::Raw {
                what,
                bytes,
                destination,
            } => deliver(what.to_string(), bytes, &destination, send),
        }
    }

    fn encode_pages(
        &mut self,
        source: &Source,
        pages: Pages,
        layout: Layout,
        config: &Config,
        send: &dyn Fn(JobEvent),
    ) -> Result<(String, Vec<u8>), String> {
        if *source == Source::Empty {
            return Err(NOTHING_OPEN.to_string());
        }
        let count = match self.load(source)? {
            Some(loaded) => match &*loaded {
                Loaded::Pdf { pages, .. } => *pages,
                Loaded::Image(_) => 1,
            },
            None => 1,
        };
        let list: Vec<usize> = match pages {
            Pages::One(page) => vec![page.min(count.saturating_sub(1))],
            Pages::All => (0..count).collect(),
        };
        send(JobEvent::Rendering(list.len()));

        // One TSPL job per page, concatenated: the printer runs them in order.
        let mut bytes = Vec::new();
        for page in &list {
            let (frame, _) = self.frame(source, *page, layout)?;
            bytes.extend(encode(&frame, config).map_err(|error| error.to_string())?);
        }
        let what = match list.len() {
            1 => "label".to_string(),
            n => format!("{n} labels"),
        };
        Ok((what, bytes))
    }

    /// Builds the black-and-white frame for one page, plus the PDF page count.
    fn frame(
        &mut self,
        source: &Source,
        page: usize,
        layout: Layout,
    ) -> Result<(Frame, Option<usize>), String> {
        if *source == Source::Empty {
            return Err(NOTHING_OPEN.to_string());
        }
        let label = layout.label;
        let options = layout.options;

        let (fitted, pages) = match self.load(source)? {
            None => {
                let card =
                    Frame::test_card(label.width, label.height).map_err(|e| e.to_string())?;
                return Ok((content::binarize(&card, options), None));
            }
            Some(loaded) => match &*loaded {
                Loaded::Image(image) => (content::fit_image(image.clone(), label, options), None),
                Loaded::Pdf { bytes, pages } => {
                    if page >= *pages {
                        return Err(format!(
                            "page {} does not exist; the PDF has {pages}",
                            page + 1
                        ));
                    }
                    let engine = self.engine()?;
                    let fitted = pdf::render_page(
                        engine,
                        bytes,
                        page,
                        label,
                        options.auto_crop,
                        options.rotation,
                    )?;
                    (fitted, Some(*pages))
                }
            },
        };

        let frame = content::place_on_label(&fitted, label)?;
        Ok((content::binarize(&frame, options), pages))
    }

    fn engine(&mut self) -> Result<&PdfEngine, String> {
        self.pdf
            .get_or_insert_with(PdfEngine::load)
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Loads (or reuses) the file; `None` for the test card.
    fn load(&mut self, source: &Source) -> Result<Option<std::rc::Rc<Loaded>>, String> {
        let Source::File(path) = source else {
            return Ok(None);
        };
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((cached, stamp, loaded)) = &self.cache {
            if cached == path && *stamp == modified {
                return Ok(Some(std::rc::Rc::clone(loaded)));
            }
        }

        let loaded = if is_pdf(path) {
            let bytes = std::fs::read(path)
                .map_err(|error| format!("read {}: {error}", display_name(path)))?;
            let pages = self.engine()?.page_count(&bytes)?;
            if pages == 0 {
                return Err(format!("{} has no pages", display_name(path)));
            }
            Loaded::Pdf { bytes, pages }
        } else {
            let image = image::open(path)
                .map_err(|error| format!("open {}: {error}", display_name(path)))?;
            Loaded::Image(image.to_luma8())
        };

        let loaded = std::rc::Rc::new(loaded);
        self.cache = Some((path.clone(), modified, std::rc::Rc::clone(&loaded)));
        Ok(Some(loaded))
    }
}

fn deliver(what: String, bytes: Vec<u8>, destination: &Destination, send: &dyn Fn(JobEvent)) {
    match destination {
        Destination::Bluetooth => send(JobEvent::ForBluetooth { what, bytes }),
        Destination::Wired(spec) => {
            let result = parse_target(spec).and_then(|target| target.send(&bytes));
            match result {
                Ok(()) => send(JobEvent::Printed {
                    what,
                    bytes: bytes.len(),
                    target: spec.clone(),
                }),
                Err(error) => send(JobEvent::Failed(error.to_string())),
            }
        }
    }
}

/// True when `path` has a `.pdf` extension.
#[must_use]
pub fn is_pdf(path: &Path) -> bool {
    has_extension(path, &["pdf"])
}

/// True when `path` is an image or PDF the app can open.
#[must_use]
pub fn is_supported(path: &Path) -> bool {
    is_pdf(path) || has_extension(path, &IMAGE_EXTENSIONS)
}

fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| list.iter().any(|e| e.eq_ignore_ascii_case(extension)))
}

/// File name for messages.
#[must_use]
pub fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Halftone;

    fn layout() -> Layout {
        Layout {
            label: LabelPx {
                width: 64,
                height: 96,
            },
            options: PrepareOptions::default(),
        }
    }

    #[test]
    fn extensions() {
        for yes in ["a.pdf", "b.PDF", "c.png", "d.JPEG", "e.tiff"] {
            assert!(is_supported(Path::new(yes)), "{yes}");
        }
        for no in ["a.txt", "b", "c.pdf.zip", ".png.exe"] {
            assert!(!is_supported(Path::new(no)), "{no}");
        }
        assert!(is_pdf(Path::new("x.Pdf")));
        assert!(!is_pdf(Path::new("x.png")));
    }

    #[test]
    fn test_card_preview_is_black_and_white_at_label_size() {
        let mut worker = Worker::default();
        let (frame, pages) = worker.frame(&Source::TestCard, 0, layout()).expect("frame");
        assert_eq!((frame.width(), frame.height()), (64, 96));
        assert!(pages.is_none());
        assert!(frame.gray().iter().all(|&v| v == 0 || v == 255));
    }

    #[test]
    fn missing_and_corrupt_files_report_their_name() {
        let mut worker = Worker::default();
        let missing = Source::File(PathBuf::from("definitely-missing-label.png"));
        let error = worker.frame(&missing, 0, layout()).expect_err("missing");
        assert!(error.contains("definitely-missing-label.png"), "{error}");

        let dir = std::env::temp_dir();
        let path = dir.join(format!("label-printer-corrupt-{}.png", std::process::id()));
        if std::fs::write(&path, b"not a png").is_ok() {
            let error = worker
                .frame(&Source::File(path.clone()), 0, layout())
                .expect_err("corrupt");
            assert!(error.contains("label-printer-corrupt"), "{error}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn printing_the_test_card_to_a_file_writes_one_job() {
        let path = std::env::current_dir()
            .expect("cwd")
            .join("target")
            .join(format!("test-print-{}.tspl", std::process::id()));
        let (tx, rx) = channel();
        let send = |event: JobEvent| {
            let _ = tx.send(event);
        };
        let mut worker = Worker::default();
        worker.handle(
            JobRequest::Print {
                source: Source::TestCard,
                pages: Pages::All,
                layout: Layout {
                    options: PrepareOptions {
                        halftone: Halftone::Photo,
                        ..PrepareOptions::default()
                    },
                    ..layout()
                },
                config: Config::by482(),
                destination: Destination::Wired(format!("file:{}", path.display())),
            },
            &send,
        );
        let events: Vec<JobEvent> = rx.try_iter().collect();
        assert!(
            matches!(events.first(), Some(JobEvent::Rendering(1))),
            "{events:?}"
        );
        match events.last() {
            Some(JobEvent::Printed { bytes, .. }) => {
                let written = std::fs::read(&path).expect("job file");
                assert_eq!(written.len(), *bytes);
                assert!(written.starts_with(b"SIZE "));
            }
            other => panic!("unexpected {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bluetooth_jobs_come_back_to_the_ui() {
        let (tx, rx) = channel();
        let send = |event: JobEvent| {
            let _ = tx.send(event);
        };
        deliver(
            "feed".to_string(),
            tspl_core::feed_job(),
            &Destination::Bluetooth,
            &send,
        );
        match rx.try_recv() {
            Ok(JobEvent::ForBluetooth { bytes, .. }) => assert_eq!(bytes, b"FORMFEED\r\n"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bad_targets_fail_with_a_message() {
        let (tx, rx) = channel();
        let send = |event: JobEvent| {
            let _ = tx.send(event);
        };
        deliver(
            "feed".to_string(),
            tspl_core::feed_job(),
            &Destination::Wired("nope".to_string()),
            &send,
        );
        assert!(matches!(rx.try_recv(), Ok(JobEvent::Failed(_))));
    }
}
