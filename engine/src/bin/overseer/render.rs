use std::collections::BTreeMap;
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
use super::board::Panel;
use super::board::{Bounds, Point, WindowId};
use super::canvas::{Canvas, Chrome};
use super::profiler::{Arrival, Histories, History};
use super::target;

pub(super) struct Preview {
    pub frame: Arc<PreviewFrame>,
    pub blur: f32,
    pub arrival: Option<Arrival>,
}

pub(super) struct FrameRequest {
    pub chrome: Chrome,
    pub target_drag: Option<target::Drag>,
    pub surface: SurfaceSize,
    pub previews: BTreeMap<WindowId, Preview>,
    pub profile_clicks: BTreeMap<WindowId, ProfileClick>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct ProfileClick {
    pub game: Option<WindowId>,
    pub bounds: Bounds,
    pub point: Point,
}

impl FrameRequest {
    fn keep_click_from(&mut self, previous: &Self) {
        for (&id, click) in &previous.profile_clicks {
            if self
                .chrome
                .board
                .window(id)
                .is_some_and(|panel| panel.attached_game() == click.game)
            {
                self.profile_clicks.entry(id).or_insert(*click);
            }
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
pub(super) type Presented = Arc<Mutex<BTreeMap<WindowId, u64>>>;
type RenderResult = Result<(), Box<dyn Error + Send + Sync>>;

pub(super) struct RenderWorker {
    work: Arc<Work>,
    thread: JoinHandle<RenderResult>,
    pub presented: Presented,
}

impl RenderWorker {
    pub fn start(
        pixels: Pixels,
        surface: SurfaceSize,
        events: EventLoopProxy<UiEvent>,
    ) -> io::Result<Self> {
        let work = Arc::new(Work::default());
        let requests = work.clone();
        let presented = Presented::default();
        let frames = presented.clone();
        let thread = thread::Builder::new()
            .name("editor render".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run(pixels, surface, &requests, &frames)
                }));
                let _ = events.send_event(UiEvent::RenderStopped);
                match result {
                    Ok(result) => result,
                    Err(panic) => resume_unwind(panic),
                }
            })?;
        Ok(Self {
            work,
            thread,
            presented,
        })
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

fn run(
    mut pixels: Pixels,
    mut surface: SurfaceSize,
    requests: &Work,
    presented: &Presented,
) -> RenderResult {
    let mut canvas = Canvas::new(&pixels);
    let mut histories = Histories::new();
    let mut last_present: Option<Instant> = None;
    let (pending, changed) = requests;

    loop {
        let gpu_pending = canvas.poll_gpu(&pixels);
        for (key, elapsed) in canvas.take_gpu_profiles() {
            for (game, history) in histories.values_mut() {
                if *game == key.game {
                    history.set_editor_gpu(key, elapsed);
                }
            }
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
            let targets: BTreeMap<_, _> = frame
                .chrome
                .board
                .windows()
                .filter(|panel| panel.kind() == Panel::Profiler)
                .filter_map(|panel| Some((panel.id, panel.attached_game()?)))
                .collect();
            histories.retain(|id, (game, _)| targets.get(id) == Some(game));
            let mut arrivals = Vec::new();
            for (id, game) in targets {
                let (_, history) = histories
                    .entry(id)
                    .or_insert_with(|| (game, History::default()));
                if let Some(click) = frame
                    .profile_clicks
                    .get(&id)
                    .filter(|click| click.game == Some(game))
                {
                    history.select(click.bounds, click.point);
                }
                history.refresh(Instant::now());
                if let Some(arrival) = frame
                    .previews
                    .get(&game)
                    .and_then(|preview| preview.arrival)
                    .filter(|&arrival| history.wants_frame(arrival))
                {
                    arrivals.push((id, arrival));
                }
            }
            let mut keys: Vec<_> = arrivals.iter().map(|(_, arrival)| arrival.key).collect();
            keys.sort_unstable_by_key(|key| (key.game, key.generation, key.frame));
            keys.dedup();
            let submission = canvas.encode(
                encoder,
                target,
                context,
                &frame.chrome,
                surface,
                frame.target_drag,
                &frame.previews,
                &histories,
                keys,
            );
            rendered = Some((submission, arrivals));
            Ok(())
        })?;
        let (mut submission, arrivals) = rendered.expect("editor render callback");
        let completed = Instant::now();
        let interval = if histories.is_empty() {
            last_present = None;
            None
        } else {
            last_present
                .replace(completed)
                .map(|last| micros(completed.duration_since(last)))
        };
        for (id, arrival) in arrivals {
            if let Some((game, history)) = histories.get_mut(&id)
                && let Some(preview) = frame.previews.get(game)
            {
                // The drawable wait can start before this preview arrives.
                let start = start.max(arrival.at);
                history.record(
                    &preview.frame,
                    arrival,
                    start,
                    micros(completed.duration_since(start)),
                    interval,
                    submission.presented.contains_key(game),
                );
            }
        }
        *presented.lock().expect("presented game frames") =
            std::mem::take(&mut submission.presented);
        canvas.after_submit(pixels.context(), submission);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::board::Board;

    fn request(game_index: usize, width: u32, click: Option<(Bounds, Point)>) -> FrameRequest {
        let size = SurfaceSize::new(width, 600);
        let mut board = Board::default();
        let area = super::super::board::viewport(size);
        let games = [board.add(Panel::Game, area), board.add(Panel::Game, area)];
        let game = games[game_index];
        let id = board.add(Panel::Profiler, area);
        board.attach_target(id, Some(game));
        let profile_clicks = click.map_or_else(BTreeMap::new, |(bounds, point)| {
            BTreeMap::from([(
                id,
                ProfileClick {
                    game: Some(game),
                    bounds,
                    point,
                },
            )])
        });
        FrameRequest {
            chrome: Chrome {
                board,
                size,
                games: BTreeMap::new(),
                focus: None,
                hovered: None,
                drag_ms: None,
                dock_target: None,
                menu_open: false,
                fullscreen: None,
                target_menu: None,
                header_menu: None,
                notice: None,
                bookmark_popup: None,
                reset_prompt: None,
                levels: BTreeMap::new(),
                picked_tile: None,
                level_tilesets: BTreeMap::new(),
                assets: BTreeMap::new(),
                explorers: BTreeMap::new(),
                active_explorer: None,
            },
            target_drag: None,
            surface: size,
            previews: BTreeMap::new(),
            profile_clicks,
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
        assert!(pending.replace(request(0, 800, first)).is_none());
        let previous = pending.replace(request(0, 900, None)).unwrap();
        assert_eq!(previous.surface.width, 800);
        assert_eq!(
            pending
                .frame
                .as_ref()
                .unwrap()
                .profile_clicks
                .get(&3)
                .map(|click| (click.bounds, click.point)),
            first
        );
        pending.replace(request(0, 950, second));
        pending.replace(request(0, 1_000, None));
        let latest = pending.frame.as_ref().unwrap();
        assert_eq!(latest.surface.width, 1_000);
        assert_eq!(
            latest
                .profile_clicks
                .get(&3)
                .map(|click| (click.bounds, click.point)),
            second
        );
        pending.replace(request(1, 1_000, None));
        assert!(pending.frame.unwrap().profile_clicks.is_empty());
    }
}
