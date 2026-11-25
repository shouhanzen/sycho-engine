use std::error::Error;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use engine::embedded::PreviewFrame;
use engine::profiling::micros;
use engine::surface::SurfaceSize;
use pixels::Pixels;
use winit::event_loop::EventLoopProxy;

use super::UiEvent;
use super::board::{Bounds, Point};
use super::canvas::{Canvas, Chrome};
use super::profiler::{Arrival, History};

pub(super) struct FrameRequest {
    pub chrome: Chrome,
    pub surface: SurfaceSize,
    pub frame: Option<Arc<PreviewFrame>>,
    pub arrival: Option<Arrival>,
    pub profile_session: u64,
    pub profile_click: Option<(Bounds, Point)>,
}

impl FrameRequest {
    fn keep_click_from(&mut self, previous: &Self) {
        if self.profile_session == previous.profile_session && self.profile_click.is_none() {
            self.profile_click = previous.profile_click;
        }
    }
}

#[derive(Default)]
struct Pending {
    frame: Option<FrameRequest>,
    closed: bool,
}

impl Pending {
    fn replace(&mut self, mut frame: FrameRequest) -> Option<FrameRequest> {
        // Redraws can be replaced while the GPU is busy. Keep the last selection
        // request until it is shown or the profiler is closed and reopened.
        if let Some(previous) = &self.frame {
            frame.keep_click_from(previous);
        }
        self.frame.replace(frame)
    }
}

type Work = (Mutex<Pending>, Condvar);
type RenderResult = Result<(), Box<dyn Error + Send + Sync>>;

pub(super) struct RenderWorker {
    work: Arc<Work>,
    thread: JoinHandle<RenderResult>,
}

impl RenderWorker {
    pub fn start(
        pixels: Pixels,
        surface: SurfaceSize,
        events: EventLoopProxy<UiEvent>,
    ) -> io::Result<Self> {
        let work = Arc::new(Work::default());
        let requests = work.clone();
        let thread = thread::Builder::new()
            .name("editor render".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| run(pixels, surface, &requests)));
                let _ = events.send_event(UiEvent::RenderStopped);
                match result {
                    Ok(result) => result,
                    Err(panic) => resume_unwind(panic),
                }
            })?;
        Ok(Self { work, thread })
    }

    pub fn submit(&self, frame: FrameRequest) {
        let (pending, changed) = &*self.work;
        let old = pending.lock().expect("editor render work").replace(frame);
        changed.notify_one();
        drop(old);
    }

    pub fn close(&self) {
        let (pending, changed) = &*self.work;
        let old = {
            let mut pending = pending.lock().expect("editor render work");
            pending.closed = true;
            pending.frame.take()
        };
        changed.notify_one();
        drop(old);
    }

    pub fn finish(self) -> RenderResult {
        self.close();
        self.thread
            .join()
            .map_err(|_| io::Error::other("editor render worker panicked"))?
    }
}

fn run(mut pixels: Pixels, mut surface: SurfaceSize, requests: &Work) -> RenderResult {
    let mut canvas = Canvas::new(&pixels);
    let mut history = History::default();
    let mut last_present: Option<Instant> = None;
    let mut profile_session = 0;
    let (pending, changed) = requests;

    loop {
        let gpu_pending = canvas.poll_gpu(&pixels);
        for (key, elapsed) in canvas.take_gpu_profiles() {
            history.set_editor_gpu(key, elapsed);
        }
        let mut frame = {
            let pending = pending.lock().expect("editor render work");
            let idle = |pending: &mut Pending| pending.frame.is_none() && !pending.closed;
            let mut pending = if gpu_pending {
                // Release shared preview surfaces even when the window has no redraw pending.
                changed
                    .wait_timeout_while(pending, Duration::from_millis(2), idle)
                    .expect("editor render work")
                    .0
            } else {
                changed
                    .wait_while(pending, idle)
                    .expect("editor render work")
            };
            if pending.closed {
                return Ok(());
            }
            let Some(frame) = pending.frame.take() else {
                continue;
            };
            frame
        };

        if frame.surface != surface {
            pixels.resize_surface(frame.surface.width, frame.surface.height)?;
            surface = frame.surface;
        }
        let start = Instant::now();
        let mut rendered = None;
        pixels.render_with(|encoder, target, context| {
            // A drawable can take a refresh interval to become available. Apply
            // any input and preview updates that arrived while waiting.
            let latest = pending
                .lock()
                .expect("editor render work")
                .frame
                .take_if(|next| next.surface == surface);
            if let Some(mut latest) = latest {
                latest.keep_click_from(&frame);
                frame = latest;
            }
            if frame.profile_session != profile_session {
                profile_session = frame.profile_session;
                history.clear();
                last_present = None;
            }
            if let Some((panel, point)) = frame.profile_click {
                history.select(panel, point);
            }
            let arrival = frame.arrival.filter(|&arrival| {
                frame.chrome.board.show_profiler && history.wants_frame(arrival)
            });
            if frame.chrome.board.show_profiler {
                history.refresh(Instant::now());
            }
            let submission = canvas.encode(
                encoder,
                target,
                context,
                frame.chrome,
                surface,
                frame.frame.as_ref(),
                &history,
                arrival.map(|arrival| arrival.key),
            );
            rendered = Some((submission, arrival));
            Ok(())
        })?;
        let (submission, arrival) = rendered.expect("editor render callback");
        let composited = submission.composited;
        canvas.after_submit(pixels.context(), submission);
        if frame.chrome.board.show_profiler {
            let completed = Instant::now();
            let interval = last_present
                .replace(completed)
                .map(|last| micros(completed.duration_since(last)));
            if let Some((arrival, frame)) = arrival.zip(frame.frame.as_deref()) {
                // The drawable wait can start before this preview arrives.
                let start = start.max(arrival.at);
                history.record(
                    frame,
                    arrival,
                    start,
                    micros(completed.duration_since(start)),
                    interval,
                    composited,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Phase;
    use crate::editor::board::Board;

    fn request(session: u64, width: u32, click: Option<(Bounds, Point)>) -> FrameRequest {
        let size = SurfaceSize::new(width, 600);
        FrameRequest {
            chrome: Chrome {
                board: Board::default(),
                size,
                phase: Phase::Running,
                connected: true,
                frame_size: None,
                master_volume: None,
                timeline: Default::default(),
                preview_focus: false,
                hovered: None,
                drag_ms: None,
                dock_target: false,
                menu_open: false,
            },
            surface: size,
            frame: None,
            arrival: None,
            profile_session: session,
            profile_click: click,
        }
    }

    #[test]
    fn redraws_keep_the_latest_frame_and_profiler_click_for_the_current_session() {
        let mut pending = Pending::default();
        let first = Some((
            Bounds::new(40.0, 60.0, 500.0, 400.0),
            Point::new(80.0, 140.0),
        ));
        let second = first.map(|(bounds, _)| (bounds, Point::new(90.0, 150.0)));
        assert!(pending.replace(request(1, 800, first)).is_none());
        let previous = pending.replace(request(1, 900, None)).unwrap();
        assert_eq!(previous.surface.width, 800);
        assert_eq!(pending.frame.as_ref().unwrap().profile_click, first);
        pending.replace(request(1, 950, second));
        pending.replace(request(1, 1_000, None));
        let latest = pending.frame.as_ref().unwrap();
        assert_eq!(latest.surface.width, 1_000);
        assert_eq!(latest.profile_click, second);
        pending.replace(request(2, 1_000, None));
        assert_eq!(pending.frame.unwrap().profile_click, None);
    }
}
