//! The gameplay simulation engine: replays a replay against a processed
//! The gameplay simulation engine: replays a replay against a processed

use crate::mods::Mods;
use crate::process::{NestedKind, ProcKind, ProcObject, ProcessedBeatmap, TAIL_LENIENCY};
use crate::replay::ReplayFrame;
use crate::score::{hit_result_ext, HitResult, Judgement, ScoreProcessor};
use crate::vec2::Vec2;

pub const MISS_WINDOW: f64 = 400.0;
const SIXTY_FRAME_TIME: f64 = 1000.0 / 60.0;
const ALLOWED_IMPORTANT_TIME_SPAN: f64 = SIXTY_FRAME_TIME * 1.2;
const FOLLOW_AREA: f32 = 2.4;
const OBJECT_RADIUS_CENTER: (f32, f32) = (256.0, 192.0);

#[derive(Clone, Copy)]
pub struct HitWindows {
    pub great: f64,
    pub ok: f64,
    pub meh: f64,
}

impl HitWindows {
    /// `OsuHitWindows.SetDifficulty` (floor - 0.5; miss window fixed at 400).
    pub fn from_od(od: f64) -> HitWindows {
        HitWindows {
            great: crate::beatmap::difficulty_range(od, 80.0, 50.0, 20.0).floor() - 0.5,
            ok: crate::beatmap::difficulty_range(od, 140.0, 100.0, 60.0).floor() - 0.5,
            meh: crate::beatmap::difficulty_range(od, 200.0, 150.0, 100.0).floor() - 0.5,
        }
    }

    pub fn window_for(&self, result: HitResult) -> f64 {
        match result {
            HitResult::Great => self.great,
            HitResult::Ok => self.ok,
            HitResult::Meh | HitResult::Miss => self.meh,
            _ => 0.0,
        }
    }

    /// `HitWindows.ResultFor` for osu (Great/Ok/Meh allowed, plus the fixed
    /// 400ms Miss window: presses beyond the meh window but within 400ms
    /// judge as Miss rather than being ignored).
    pub fn result_for(&self, time_offset: f64) -> HitResult {
        let time_offset = time_offset.abs();
        if time_offset <= self.great {
            HitResult::Great
        } else if time_offset <= self.ok {
            HitResult::Ok
        } else if time_offset <= self.meh {
            HitResult::Meh
        } else if time_offset <= MISS_WINDOW {
            HitResult::Miss
        } else {
            HitResult::None
        }
    }

    /// `HitWindows.CanBeHit` (against the Meh window).
    pub fn can_be_hit(&self, time_offset: f64) -> bool {
        time_offset <= self.meh
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub struct TimelineEntry {
    pub time: f64,
    pub object_index: usize,
    pub label: String,
    pub result: HitResult,
    pub combo: i32,
    pub score: i64,
    pub classic_score: i64,
    pub accuracy: f64,
    pub unstable_rate: Option<f64>,
    pub time_offset: f64,
}

/// Visual state snapshot of one spinner, recorded per game frame.
#[derive(Clone, Copy, Debug)]
pub struct SpinnerSnap {
    pub object_index: usize,
    /// Raw accumulated visual rotation in degrees (`currentRotation`, can be
    /// negative; the displayed disc angle damps toward this).
    pub visual_rotation: f32,
    /// Score-side rotation (`SpinnerSpinHistory.TotalRotation`), degrees.
    pub total_rotation: f32,
    pub tracking: bool,
}

/// Visual state snapshot of one game frame (60fps frame-stable playback).
/// Recorded after the stability iterations settle for the frame.
#[derive(Clone, Debug)]
pub struct FrameSnap {
    /// Reference clock time at the end of this game frame.
    pub time: f64,
    /// Interpolated cursor position at `time`.
    pub cursor: Vec2,
    pub left: bool,
    pub right: bool,
    /// Smoke button state (key overlay only; no gameplay effect).
    pub smoke: bool,
    /// Slider tracking states for objects in their visual lifespan.
    pub sliders: Vec<(usize, bool)>,
    pub spinners: Vec<SpinnerSnap>,
}

#[derive(Default)]
struct SliderRuntime {
    tracking: bool,
    time_to_accept_any_key_after: Option<f64>,
    last_pressed_actions: Vec<Action>,
    head_hit_action: Option<Action>,
}

struct SpinnerRuntime {
    judged: bool,
    result: Option<HitResult>,
    last_angle: Option<f32>,
    tracking: bool,
    completed_spins: i32,
    total_accumulated_rotation: f32,
    total_accumulated_rotation_at_last_completion: f32,
    current_spin_max_rotation: f32,
    last_report_time: f64,
    completed_full_spins: i32,
    next_tick: usize,
}

impl SpinnerRuntime {
    fn new() -> SpinnerRuntime {
        SpinnerRuntime {
            judged: false,
            result: None,
            last_angle: None,
            tracking: false,
            completed_spins: 0,
            total_accumulated_rotation: 0.0,
            total_accumulated_rotation_at_last_completion: 0.0,
            current_spin_max_rotation: 0.0,
            last_report_time: f64::NEG_INFINITY,
            completed_full_spins: 0,
            next_tick: 0,
        }
    }

    fn current_spin_rotation(&self) -> f32 {
        self.total_accumulated_rotation - self.total_accumulated_rotation_at_last_completion
    }

    fn total_rotation(&self) -> f32 {
        360.0 * self.completed_spins as f32 + self.current_spin_max_rotation
    }

    fn report_delta(&mut self, current_time: f64, delta: f32) {
        if delta == 0.0 {
            return;
        }

        self.total_accumulated_rotation += delta;

        if current_time >= self.last_report_time {
            let csr = self.current_spin_rotation();
            self.current_spin_max_rotation = self.current_spin_max_rotation.max(csr.abs());

            while self.current_spin_max_rotation >= 360.0 {
                let direction = csr.signum();

                self.completed_spins += 1;
                self.total_accumulated_rotation_at_last_completion += direction * 360.0;
                self.current_spin_max_rotation = self.current_spin_rotation().abs();
            }
        }

        self.last_report_time = current_time;
    }
}

struct ObjectRuntime {
    /// Circle: judged/result. Slider: body judged/result. Spinner: final.
    judged: bool,
    result: Option<HitResult>,
    /// Per-nested judging state for sliders (aligned with nested order).
    nested_judged: Vec<bool>,
    nested_results: Vec<Option<HitResult>>,
    /// Drawable container order: ticks (time order), repeats (time order), tail, head.
    container_order: Vec<usize>,
    slider: SliderRuntime,
    spinner: Option<SpinnerRuntime>,
}

pub struct Engine {
    objects: Vec<ProcObject>,
    windows: HitWindows,
    classic: bool,
    rate: f64,
    /// Beatmap HP drain rate (`Difficulty.HPDrainRate`, modded) - drives
    /// the miss-amount table and the drain-rate solver target.
    hp_drain: f64,
    /// `[Events]` break periods (`DrainingHealthProcessor` no-drain input).
    breaks: Vec<(f64, f64)>,
    /// `IHasComboInformation.LastInCombo` per object: the NEXT object
    /// starts a new combo (`UpdateComboInformation` marks the previous
    /// one). The final object never gets it (nothing follows).
    last_in_combo: Vec<bool>,
    pub score: ScoreProcessor,

    runtime: Vec<ObjectRuntime>,

    cursor: Vec2,
    pressed: Vec<Action>,

    update_order: Vec<usize>,
    input_order: Vec<usize>,

    pub timeline: Vec<TimelineEntry>,
    /// Per-game-frame visual snapshots (filled by `run`).
    pub snapshots: Vec<FrameSnap>,
    /// Health processor output (filled at the end of `run`).
    pub health: crate::health::HealthInfo,
}

/// Slider path position at overall slider progress [0..1], mirroring the
/// direction on odd spans (`IHasPathWithRepeats.CurvePositionAt`).
fn path_position_at(obj: &ProcObject, progress: f64) -> Vec2 {
    match &obj.kind {
        ProcKind::Slider { path, span_count, .. } => {
            let sc = *span_count as f64;
            let mut p = (progress * sc) % 1.0;
            let span = (progress * sc) as i32;
            if span % 2 == 1 {
                p = 1.0 - p;
            }
            path.position_at(p)
        }
        _ => Vec2::ZERO,
    }
}

/// Interpolated cursor position at time `t` (`FramedReplayInputHandler`
/// semantics: exact at a consumed frame, linear towards the next otherwise).
fn interpolate_cursor(frames: &[ReplayFrame], next_frame: usize, t: f64) -> Vec2 {
    if next_frame > 0 && frames[next_frame - 1].time == t {
        let f = &frames[next_frame - 1];
        return Vec2::new(f.position.0, f.position.1);
    }

    let start_idx = next_frame.saturating_sub(1);
    let start = &frames[start_idx];
    match frames.get(next_frame) {
        Some(end) => {
            let span = end.time - start.time;
            if span <= 0.0 {
                Vec2::new(start.position.0, start.position.1)
            } else {
                Vec2::lerp(
                    Vec2::new(start.position.0, start.position.1),
                    Vec2::new(end.position.0, end.position.1),
                    (t - start.time) / span,
                )
            }
        }
        None => Vec2::new(start.position.0, start.position.1),
    }
}

impl Engine {
    pub fn new(beatmap: ProcessedBeatmap, mods: &Mods) -> Engine {
        let windows = HitWindows::from_od(beatmap.difficulty.od as f64);

        let mut runtime = Vec::with_capacity(beatmap.objects.len());
        for obj in &beatmap.objects {
            let (nested_len, container_order) = match &obj.kind {
                ProcKind::Slider { nested, .. } => {
                    let mut ticks: Vec<usize> = Vec::new();
                    let mut repeats: Vec<usize> = Vec::new();
                    let mut tail = None;
                    let mut head = None;
                    for (i, n) in nested.iter().enumerate() {
                        match n.kind {
                            NestedKind::Head => head = Some(i),
                            NestedKind::Tail => tail = Some(i),
                            NestedKind::Tick => ticks.push(i),
                            NestedKind::Repeat => repeats.push(i),
                        }
                    }
                    let mut order = Vec::new();
                    order.extend(ticks);
                    order.extend(repeats);
                    if let Some(t) = tail {
                        order.push(t);
                    }
                    if let Some(h) = head {
                        order.push(h);
                    }
                    (nested.len(), order)
                }
                _ => (0, Vec::new()),
            };

            runtime.push(ObjectRuntime {
                judged: false,
                result: None,
                nested_judged: vec![false; nested_len],
                nested_results: vec![None; nested_len],
                container_order,
                slider: SliderRuntime::default(),
                spinner: match &obj.kind {
                    ProcKind::Spinner { .. } => Some(SpinnerRuntime::new()),
                    _ => None,
                },
            });
        }

        let mut update_order: Vec<usize> = (0..beatmap.objects.len()).collect();
        update_order.sort_by(|&a, &b| {
            beatmap.objects[b]
                .start_time
                .partial_cmp(&beatmap.objects[a].start_time)
                .unwrap()
                // HitObjectContainer ties: CompareReverseChildID (later-added
                // draws first in the sorted list, and updates follow it).
                .then(b.cmp(&a))
        });

        let mut input_order: Vec<usize> = (0..beatmap.objects.len()).collect();
        input_order.sort_by(|&a, &b| {
            beatmap.objects[a]
                .start_time
                .partial_cmp(&beatmap.objects[b].start_time)
                .unwrap()
                .then(a.cmp(&b))
        });

        let last_in_combo: Vec<bool> = (0..beatmap.objects.len())
            .map(|i| beatmap.objects.get(i + 1).is_some_and(|next| next.new_combo))
            .collect();

        Engine {
            objects: beatmap.objects,
            windows,
            classic: mods.classic,
            rate: mods.rate,
            hp_drain: beatmap.difficulty.hp as f64,
            breaks: beatmap.breaks,
            last_in_combo,
            score: ScoreProcessor::new(mods.score_multiplier_v2()),
            runtime,
            cursor: Vec2::new(256.0, 192.0),
            pressed: Vec::new(),
            update_order,
            input_order,
            timeline: Vec::new(),
            snapshots: Vec::new(),
            health: crate::health::HealthInfo::default(),
        }
    }

    pub fn objects(&self) -> &[ProcObject] {
        &self.objects
    }

    /// The OD hit windows (great/ok/meh, ms) used for this run.
    pub fn windows(&self) -> HitWindows {
        self.windows
    }

    // ------------------------------------------------------------------
    // Judgement definitions
    // ------------------------------------------------------------------

    fn judgement_for(&self, obj_idx: usize, nested_idx: Option<usize>) -> (Judgement, bool) {
        match (&self.objects[obj_idx].kind, nested_idx) {
            (ProcKind::Circle, _) => (Judgement { max_result: HitResult::Great }, true),
            (ProcKind::Slider { nested, .. }, Some(n)) => match nested[n].kind {
                NestedKind::Head => {
                    if self.classic {
                        (Judgement { max_result: HitResult::LargeTickHit }, true)
                    } else {
                        (Judgement { max_result: HitResult::Great }, true)
                    }
                }
                NestedKind::Tick | NestedKind::Repeat => {
                    (Judgement { max_result: HitResult::LargeTickHit }, false)
                }
                NestedKind::Tail => {
                    if self.classic {
                        (Judgement { max_result: HitResult::SmallTickHit }, false)
                    } else {
                        (Judgement { max_result: HitResult::SliderTailHit }, false)
                    }
                }
            },
            (ProcKind::Slider { .. }, None) => {
                if self.classic {
                    (Judgement { max_result: HitResult::Great }, false)
                } else {
                    (Judgement { max_result: HitResult::IgnoreHit }, false)
                }
            }
            (ProcKind::Spinner { .. }, None) => (Judgement { max_result: HitResult::Great }, false),
            (ProcKind::Spinner { .. }, Some(n)) => {
                // n here is a tick index into the spinner tick list.
                if let ProcKind::Spinner { spins_required, .. } = &self.objects[obj_idx].kind {
                    let spins_for_bonus = spins_required + 2;
                    if n < spins_for_bonus {
                        (Judgement { max_result: HitResult::SmallBonus }, false)
                    } else {
                        (Judgement { max_result: HitResult::LargeBonus }, false)
                    }
                } else {
                    unreachable!()
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Autoplay simulation (max statistics)
    // ------------------------------------------------------------------

    pub fn simulate_autoplay(&mut self) {
        let mut order: Vec<(usize, Option<usize>)> = Vec::new();
        for i in 0..self.objects.len() {
            match &self.objects[i].kind {
                ProcKind::Slider { nested, .. } => {
                    for n in 0..nested.len() {
                        order.push((i, Some(n)));
                    }
                }
                ProcKind::Spinner { ticks, .. } => {
                    for n in 0..ticks.len() {
                        order.push((i, Some(n)));
                    }
                }
                ProcKind::Circle => {}
            }
            order.push((i, None));
        }

        for (obj_idx, nested_idx) in order {
            let (judgement, has_windows) = self.judgement_for(obj_idx, nested_idx);
            self.score
                .apply_result(judgement, judgement.max_result, 0.0, 0.0, has_windows, 1.0);
        }

        self.score.store_maxima();
        self.score.reset();
    }

    // ------------------------------------------------------------------
    // Main loop
    // ------------------------------------------------------------------

    pub fn run(&mut self, frames: &[ReplayFrame]) {
        self.simulate_autoplay();

        let first_start = self.objects.first().map(|o| o.start_time).unwrap_or(0.0);
        let first_preempt = self.objects.first().map(|o| o.time_preempt).unwrap_or(1200.0);
        let gameplay_start = first_start - first_preempt.max(2000.0);

        let last_needed = self
            .objects
            .iter()
            .map(|o| o.end_time)
            .fold(f64::NEG_INFINITY, f64::max)
            + MISS_WINDOW
            + 100.0;

        // Model of 60fps frame-stable playback (the exact FrameStabilityContainer
        // algorithm): the reference clock advances 1000/60 * rate ms per game
        // frame. Within a game frame, the container performs stability
        // iterations: each proposes min-clamped time (at most +1000/60 ahead of
        // the manual clock when the reference jumped too far, else the reference
        // itself), consumes at most one replay frame (updating at its exact
        // time), otherwise updates at the frame-span-clamped proposal. The loop
        // repeats while the manual clock is still behind the reference.
        // Align the 60Hz reference grid to map-time zero (phase-0
        // lattice). The grid phase decides which instents get evaluated,
        // and threshold-gated judgements (slider tails inside the -36ms
        // leniency with the cursor leaving the follow circle in the same
        // few ms) are phase-sensitive: the phase must be canonical to
        // match lazer's clock lattice, whose frames tick at multiples of
        // 1000/60 from zero.
        let grid_start = (gameplay_start / SIXTY_FRAME_TIME).ceil() * SIXTY_FRAME_TIME;
        let mut reference = grid_start;
        let mut manual = grid_start;
        let mut next_frame = 0usize;
        let mut ticks: u64 = 0;

        loop {
            if reference > last_needed && next_frame >= frames.len() {
                break;
            }
            ticks += 1;
            if ticks > 50_000_000 {
                break; // safety
            }

            reference += SIXTY_FRAME_TIME * self.rate;

            // Stability iterations until the manual clock reaches the reference.
            loop {
                let diff = reference - manual;
                let proposed = if diff.abs() > ALLOWED_IMPORTANT_TIME_SPAN {
                    manual + SIXTY_FRAME_TIME
                } else {
                    reference
                };

                let update_time = if next_frame < frames.len() && frames[next_frame].time <= proposed {
                    let t = frames[next_frame].time;
                    next_frame += 1;
                    t
                } else {
                    let frame_start = if next_frame == 0 {
                        f64::NEG_INFINITY
                    } else {
                        frames[next_frame - 1].time
                    };
                    let frame_end = frames.get(next_frame).map(|f| f.time).unwrap_or(f64::INFINITY);
                    proposed.min(frame_end).max(frame_start)
                };

                self.tick_at(frames, next_frame, update_time);
                manual = update_time;

                // timeBehind == 0 -> stop catch-up for this game frame.
                if update_time == reference {
                    break;
                }
            }

            // Visual snapshot at the end of the game frame (the state the
            // draw would present).
            let cursor = interpolate_cursor(frames, next_frame, reference);
            let (left, right, smoke) = match frames.get(next_frame.saturating_sub(1)) {
                Some(f) if next_frame > 0 => (f.left, f.right, f.smoke),
                _ => (false, false, false),
            };

            let mut sliders = Vec::new();
            let mut spinners = Vec::new();
            for (i, obj) in self.objects.iter().enumerate() {
                let in_visual_span = reference >= obj.start_time - obj.time_preempt
                    && reference <= obj.end_time + MISS_WINDOW;
                if !in_visual_span {
                    continue;
                }
                match &obj.kind {
                    ProcKind::Slider { .. } => {
                        sliders.push((i, self.runtime[i].slider.tracking));
                    }
                    ProcKind::Spinner { .. } => {
                        if let Some(rt) = &self.runtime[i].spinner {
                            spinners.push(SpinnerSnap {
                                object_index: i,
                                visual_rotation: rt.total_accumulated_rotation,
                                total_rotation: rt.total_rotation(),
                                tracking: rt.tracking,
                            });
                        }
                    }
                    ProcKind::Circle => {}
                }
            }

            self.snapshots.push(FrameSnap {
                time: reference,
                cursor,
                left,
                right,
                smoke,
                sliders,
                spinners,
            });
        }

        // Health processor pass (`HealthProcessor` + `DrainingHealthProcessor`
        // + `OsuHealthProcessor`): drain rate solved off the perfect-play
        // simulation, then the live timeline walked into an HP curve.
        self.health = self.build_health();
    }

    /// Runs one update at `update_time`; `next_frame` is the index of the next
    /// unconsumed replay frame (so `next_frame - 1` is the current one).
    fn tick_at(&mut self, frames: &[ReplayFrame], next_frame: usize, update_time: f64) {
        let frame_consumed = next_frame > 0
            && frames[next_frame - 1].time == update_time
            && next_frame > 0;

        // Cursor at this time (interpolated between the current and next frame).
        let cursor = if frame_consumed {
            let f = &frames[next_frame - 1];
            Vec2::new(f.position.0, f.position.1)
        } else {
            // Interpolate towards the next frame from the current one.
            let start_idx = next_frame.saturating_sub(1);
            let start = &frames[start_idx];
            match frames.get(next_frame) {
                Some(end) => {
                    let span = end.time - start.time;
                    if span <= 0.0 {
                        Vec2::new(start.position.0, start.position.1)
                    } else {
                        Vec2::lerp(
                            Vec2::new(start.position.0, start.position.1),
                            Vec2::new(end.position.0, end.position.1),
                            (update_time - start.time) / span,
                        )
                    }
                }
                None => Vec2::new(start.position.0, start.position.1),
            }
        };

        let new_pressed = if frame_consumed {
            let f = &frames[next_frame - 1];
            let mut actions = Vec::new();
            if f.left {
                actions.push(Action::Left);
            }
            if f.right {
                actions.push(Action::Right);
            }
            Some(actions)
        } else {
            None
        };

        self.tick(update_time, cursor, new_pressed);
    }

    fn tick(&mut self, t: f64, cursor: Vec2, new_pressed: Option<Vec<Action>>) {
        self.cursor = cursor;

        if let Some(new_pressed) = new_pressed {
            // The input state updates BEFORE events dispatch (framework
            // semantics), so handlers (e.g. PostProcessHeadJudgement's
            // tracking re-establish) see the new press.
            let old_pressed = std::mem::replace(&mut self.pressed, new_pressed);

            // Press events dispatch in action order (left before right).
            for action in [Action::Left, Action::Right] {
                if self.pressed.contains(&action) && !old_pressed.contains(&action) {
                    self.dispatch_press(t, action);
                }
            }
        }

        let order = self.update_order.clone();
        for obj_idx in order {
            if t < self.objects[obj_idx].start_time - self.objects[obj_idx].time_preempt {
                continue;
            }

            match &self.objects[obj_idx].kind {
                ProcKind::Circle => self.circle_update(obj_idx, t),
                ProcKind::Slider { .. } => self.slider_update(obj_idx, t),
                ProcKind::Spinner { .. } => self.spinner_update(obj_idx, t),
            }
        }
    }

    // ------------------------------------------------------------------
    // Input dispatch
    // ------------------------------------------------------------------

    fn dispatch_press(&mut self, t: f64, action: Action) {
        for obj_idx in self.input_order.clone() {
            let obj = &self.objects[obj_idx];
            if t < obj.start_time - obj.time_preempt {
                // Not alive yet - receptor doesn't exist.
                continue;
            }

            // Only circles and slider heads have hit receptors.
            let nested_head = match &obj.kind {
                ProcKind::Circle => None,
                ProcKind::Slider { nested, .. } => {
                    let mut head = None;
                    for (i, n) in nested.iter().enumerate() {
                        if n.kind == NestedKind::Head {
                            head = Some(i);
                            break;
                        }
                    }
                    head
                }
                ProcKind::Spinner { .. } => continue,
            };

            let receptor_idx = match nested_head {
                Some(h) => h,
                None => 0usize,
            };

            let (is_head, receptor_pos, radius) = match &obj.kind {
                ProcKind::Circle => (false, obj.stacked_position(), obj.radius),
                ProcKind::Slider { nested: _, .. } => {
                    (true, obj.stacked_position(), obj.radius)
                }
                _ => continue,
            };
            let _ = receptor_idx;

            // CanBeHit
            let can_be_hit = if is_head {
                if self.classic {
                    // blockInputToObjectsUnderSliderHead: CanBeHit = !slider.AllJudged
                    !self.slider_all_judged(obj_idx)
                } else {
                    !self.nested_is_judged(obj_idx, head_index_of(&self.objects[obj_idx]).unwrap())
                }
            } else {
                !self.runtime[obj_idx].judged
            };

            if !can_be_hit {
                continue;
            }

            // IsHovered: within circle radius.
            let hovered = Vec2::distance(self.cursor, receptor_pos) <= radius;
            if !hovered {
                continue;
            }

            // The receptor consumes the press and attempts the hit.
            self.circle_hit_attempt(obj_idx, is_head, t, action);
            break;
        }
    }

    fn circle_hit_attempt(&mut self, obj_idx: usize, is_head: bool, t: f64, action: Action) {
        // HitReceptor records the FIRST hover-press action regardless of the
        // judgement outcome (HitAction ??= action), which anchors the slider
        // tracking key constraint.
        if is_head && self.runtime[obj_idx].slider.head_hit_action.is_none() {
            self.runtime[obj_idx].slider.head_hit_action = Some(action);
        }

        // UpdateResult: already-judged objects never re-judge.
        if is_head {
            let head_idx = head_index_of(&self.objects[obj_idx]).unwrap();
            if self.runtime[obj_idx].nested_judged[head_idx] {
                return;
            }
        } else if self.runtime[obj_idx].judged {
            return;
        }

        let start_time = if is_head {
            head_start(&self.objects[obj_idx])
        } else {
            self.objects[obj_idx].start_time
        };
        let time_offset = t - start_time;

        // DrawableSliderHead.ResultFor override for classic behaviour.
        let result = if is_head && self.classic {
            let base = self.windows.result_for(time_offset);
            if hit_result_ext::is_hit(base) {
                HitResult::LargeTickHit
            } else {
                HitResult::LargeTickMiss
            }
        } else {
            self.windows.result_for(time_offset)
        };

        let click_action = self.check_hittable(obj_idx, is_head, t, result);
        if result == HitResult::None || click_action != ClickAction::Hit {
            // Shake - no judgement.
            if is_head {
                // PostProcessHeadJudgement still runs (but requires judged hit).
            }
            return;
        }

        // Apply the result.
        let (max_judgement, has_windows) = if is_head {
            if self.classic {
                (Judgement { max_result: HitResult::LargeTickHit }, true)
            } else {
                (Judgement { max_result: HitResult::Great }, true)
            }
        } else {
            (Judgement { max_result: HitResult::Great }, true)
        };

        let label = if is_head { "head" } else { "circle" };
        if is_head {
            let head_idx = head_index_of(&self.objects[obj_idx]).unwrap();

            // Nested slider heads are DrawableHitCircle too: their OnNewResult
            // bubbles to the playfield, so HandleHit's forced misses apply
            // before the head's own scoring (same ordering as circles; fires
            // for any judged result, including Miss).
            if !self.classic {
                self.start_time_ordered_handle_hit(obj_idx, t);
            }

            self.set_nested_result(obj_idx, head_idx, result, t, max_judgement, has_windows, label);
            self.post_process_head_judgement(obj_idx, t);
        } else {
            self.runtime[obj_idx].judged = true;
            self.runtime[obj_idx].result = Some(result);

            // `OsuPlayfield.onNewResult` -> hitPolicy.HandleHit runs while the
            // hit's result propagates, so its forced misses reach the score
            // processor BEFORE the triggering hit itself.
            if !self.classic {
                self.start_time_ordered_handle_hit(obj_idx, t);
            }

            self.apply_score(
                obj_idx,
                None,
                max_judgement,
                result,
                t,
                time_offset,
                has_windows,
                label,
            );
        }
    }

    fn post_process_head_judgement(&mut self, obj_idx: usize, t: f64) {
        let head_idx = head_index_of(&self.objects[obj_idx]).unwrap();
        let head_hit = self.runtime[obj_idx].nested_results[head_idx]
            .map(|r| hit_result_ext::is_hit(r))
            .unwrap_or(false);
        if !head_hit {
            return;
        }

        if !self.is_mouse_in_follow_area(obj_idx, t, true) {
            return;
        }

        let mouse_in_slider = self.cursor - self.objects[obj_idx].stacked_position();

        // Determine whether all ticks so far are in range of the expanded follow area.
        let mut all_ticks_in_range = true;

        {
            let obj = &self.objects[obj_idx];
            let (duration, radius) = match &obj.kind {
                ProcKind::Slider { duration, .. } => (*duration, obj.radius * FOLLOW_AREA),
                _ => return,
            };

            let nested = nested_of(obj);
            for (i, n) in nested.iter().enumerate() {
                if self.runtime[obj_idx].nested_judged[i] {
                    continue;
                }
                if n.time > t {
                    break;
                }

                let object_progress = ((n.time - obj.start_time) / duration).clamp(0.0, 1.0);
                let object_position = path_position_at(obj, object_progress);

                if (object_position - mouse_in_slider).length_squared() > radius * radius {
                    all_ticks_in_range = false;
                    break;
                }
            }
        }

        // Collect the pending nested objects to judge, then apply.
        let mut to_judge: Vec<usize> = Vec::new();
        {
            let nested = nested_of(&self.objects[obj_idx]);
            for (i, n) in nested.iter().enumerate() {
                if self.runtime[obj_idx].nested_judged[i] {
                    continue;
                }
                if n.time > t {
                    break;
                }
                to_judge.push(i);
            }
        }

        for i in to_judge {
            if all_ticks_in_range {
                self.nested_hit_forcefully(obj_idx, i, t);
            } else {
                self.nested_miss_forcefully(obj_idx, i, t);
            }
        }

        // Enable tracking if appropriate.
        self.update_tracking_with(obj_idx, t, all_ticks_in_range || self.is_mouse_in_follow_area(obj_idx, t, false));
    }

    // ------------------------------------------------------------------
    // Hit policies
    // ------------------------------------------------------------------

    fn alive_top_level(&self, t: f64) -> Vec<usize> {
        // AliveObjects: ordered by start time (stable).
        self.input_order
            .iter()
            .copied()
            .filter(|&i| t >= self.objects[i].start_time - self.objects[i].time_preempt)
            .collect()
    }

    fn check_hittable(&self, obj_idx: usize, is_head: bool, time: f64, result: HitResult) -> ClickAction {
        if self.classic {
            self.legacy_check_hittable(obj_idx, is_head, time, result)
        } else {
            self.start_time_ordered_check_hittable(obj_idx, is_head, time, result)
        }
    }

    /// `LegacyHitPolicy.CheckHittable`.
    fn legacy_check_hittable(&self, obj_idx: usize, is_head: bool, time: f64, result: HitResult) -> ClickAction {
        let alive = self.alive_top_level(time);
        let start_time = if is_head {
            head_start(&self.objects[obj_idx])
        } else {
            self.objects[obj_idx].start_time
        };

        let index = if !is_head {
            alive.iter().position(|&i| i == obj_idx)
        } else {
            None
        };

        if let Some(index) = index {
            if index > 0 {
                let prev = alive[index - 1];
                if self.objects[prev].stack_height > 0 && !self.top_level_all_judged(prev) {
                    return ClickAction::Ignore;
                }
            }
        }

        if result == HitResult::None {
            return ClickAction::Shake;
        }

        for &test in &alive {
            if self.top_level_all_judged(test) {
                continue;
            }

            // Nested objects (slider heads) never match a top-level object, so
            // the loop checks every alive top-level object for them.
            if !is_head && test == obj_idx {
                break;
            }

            if self.objects[test].end_time + 3.0 < start_time {
                return ClickAction::Shake;
            }
        }

        if (start_time - time).abs() < MISS_WINDOW {
            ClickAction::Hit
        } else {
            ClickAction::Shake
        }
    }

    /// `StartTimeOrderedHitPolicy.CheckHittable`.
    fn start_time_ordered_check_hittable(&self, obj_idx: usize, is_head: bool, time: f64, result: HitResult) -> ClickAction {
        let target = if is_head {
            head_start(&self.objects[obj_idx])
        } else {
            self.objects[obj_idx].start_time
        };

        let mut blocking: Option<(usize, Option<usize>)> = None;
        for &i in &self.input_order {
            let obj = &self.objects[i];
            if obj.start_time >= target {
                break;
            }

            // The top-level object itself.
            if matches!(obj.kind, ProcKind::Circle) {
                blocking = Some((i, None));
            }

            if let ProcKind::Slider { nested, .. } = &obj.kind {
                for (n, nested_obj) in nested.iter().enumerate() {
                    if nested_obj.time >= target {
                        break;
                    }
                    if nested_obj.kind == NestedKind::Head {
                        blocking = Some((i, Some(n)));
                    }
                }
            }
        }

        if let Some((b_obj, b_nested)) = blocking {
            let judged = match b_nested {
                None => self.runtime[b_obj].judged,
                Some(n) => self.runtime[b_obj].nested_judged[n],
            };
            let b_start = match b_nested {
                None => self.objects[b_obj].start_time,
                Some(n) => nested_of(&self.objects[b_obj])[n].time,
            };
            if !judged && time < b_start {
                return ClickAction::Shake;
            }
        }

        if result == HitResult::None {
            return ClickAction::Shake;
        }

        ClickAction::Hit
    }

    /// `StartTimeOrderedHitPolicy.HandleHit` - called after a top-level circle
    /// was hit; misses all prior unjudged circle-like objects (incl. slider heads).
    fn start_time_ordered_handle_hit(&mut self, obj_idx: usize, t: f64) {
        let target = self.objects[obj_idx].start_time;

        // Collect the objects to miss first (borrow-safe).
        let mut to_miss: Vec<(usize, Option<usize>)> = Vec::new();

        for &i in &self.input_order {
            let obj = &self.objects[i];
            if obj.start_time >= target {
                break;
            }

            if matches!(obj.kind, ProcKind::Circle) && !self.runtime[i].judged {
                to_miss.push((i, None));
            }

            if let ProcKind::Slider { nested, .. } = &obj.kind {
                for (n, nested_obj) in nested.iter().enumerate() {
                    if nested_obj.time >= target {
                        break;
                    }
                    if nested_obj.kind == NestedKind::Head && !self.runtime[i].nested_judged[n] {
                        to_miss.push((i, Some(n)));
                    }
                }
            }
        }

        for (i, n) in to_miss {
            let (judgement, has_windows) = self.judgement_for(i, n);
            let result = judgement.min_result();
            if let Some(n) = n {
                let label = "head";
                self.set_nested_result(i, n, result, t, judgement, has_windows, label);
            } else {
                self.runtime[i].judged = true;
                self.runtime[i].result = Some(result);
                let time_offset = t - self.objects[i].start_time;
                self.apply_score(i, None, judgement, result, t, time_offset, has_windows, "circle");
            }
        }
    }

    // ------------------------------------------------------------------
    // Object updates
    // ------------------------------------------------------------------

    fn circle_update(&mut self, obj_idx: usize, t: f64) {
        if self.runtime[obj_idx].judged {
            return;
        }

        let time_offset = t - self.objects[obj_idx].start_time;
        if !self.windows.can_be_hit(time_offset) {
            let judgement = Judgement { max_result: HitResult::Great };
            let result = judgement.min_result();
            self.runtime[obj_idx].judged = true;
            self.runtime[obj_idx].result = Some(result);
            self.apply_score(obj_idx, None, judgement, result, t, time_offset, true, "circle");
        }
    }

    fn slider_update(&mut self, obj_idx: usize, t: f64) {
        // SliderInputManager.Update -> updateTracking
        self.update_tracking(obj_idx, t);
        // Nested updates in drawable container order (ticks, repeats, tail, head).
        let container_order = self.runtime[obj_idx].container_order.clone();
        for nested_idx in container_order {
            self.try_judge_nested(obj_idx, nested_idx, t);
        }

        // DrawableSlider.CheckForResult - final slider judgement.
        let tail_idx = self.find_nested(obj_idx, NestedKind::Tail);
        let tail_judged = tail_idx.map(|ti| self.runtime[obj_idx].nested_judged[ti]).unwrap_or(true);

        if tail_judged && t >= self.objects[obj_idx].end_time && !self.runtime[obj_idx].judged {
            let (judgement, has_windows) = self.judgement_for(obj_idx, None);

            let result = if self.classic {
                // Proportional to nested hit fraction.
                let nested_len = self.runtime[obj_idx].nested_judged.len();
                let hit_ticks = self
                    .runtime[obj_idx]
                    .nested_results
                    .iter()
                    .filter(|r| r.map(|res| hit_result_ext::is_hit(res)).unwrap_or(false))
                    .count();

                if hit_ticks == nested_len {
                    HitResult::Great
                } else if hit_ticks == 0 {
                    HitResult::Miss
                } else {
                    let hit_fraction = hit_ticks as f64 / nested_len as f64;
                    if hit_fraction >= 0.5 {
                        HitResult::Ok
                    } else {
                        HitResult::Meh
                    }
                }
            } else {
                let any_hit = self.runtime[obj_idx]
                    .nested_results
                    .iter()
                    .any(|r| r.map(|res| hit_result_ext::is_hit(res)).unwrap_or(false));
                if any_hit {
                    judgement.max_result
                } else {
                    judgement.min_result()
                }
            };

            self.runtime[obj_idx].judged = true;
            self.runtime[obj_idx].result = Some(result);
            self.apply_score(obj_idx, None, judgement, result, t, 0.0, has_windows, "slider");
        }
    }

    fn try_judge_nested(&mut self, obj_idx: usize, nested_idx: usize, t: f64) {
        if self.runtime[obj_idx].nested_judged[nested_idx] {
            return;
        }

        let (nested_time, kind) = {
            let nested = nested_of(&self.objects[obj_idx]);
            (nested[nested_idx].time, nested[nested_idx].kind)
        };
        let time_offset = t - nested_time;

        match kind {
            NestedKind::Tick | NestedKind::Repeat => {
                if time_offset < 0.0 {
                    return;
                }
            }
            NestedKind::Tail => {
                if time_offset < TAIL_LENIENCY {
                    return;
                }

                // The tail can only activate after all previous ticks/repeats.
                let nested = nested_of(&self.objects[obj_idx]);
                let last_tick = nested
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, n)| matches!(n.kind, NestedKind::Tick | NestedKind::Repeat));
                if let Some((li, _)) = last_tick {
                    if !self.runtime[obj_idx].nested_judged[li] {
                        return;
                    }
                }
            }
            NestedKind::Head => {
                // Head is a circle: handle its miss below (circle-like window).
                let head_idx = nested_idx;
                if !self.windows.can_be_hit(time_offset) {
                    let (judgement, has_windows) = self.judgement_for(obj_idx, Some(head_idx));
                    let result = judgement.min_result();
                    self.set_nested_result(obj_idx, head_idx, result, t, judgement, has_windows, "head");
                }
                return;
            }
        }

        let head_idx = head_index_of(&self.objects[obj_idx]).unwrap();
        if !self.runtime[obj_idx].nested_judged[head_idx] {
            return;
        }

        let tracking = self.runtime[obj_idx].slider.tracking;
        if tracking {
            self.nested_hit_forcefully(obj_idx, nested_idx, t);
        } else if time_offset >= 0.0 {
            self.nested_miss_forcefully(obj_idx, nested_idx, t);
        }
    }

    fn nested_hit_forcefully(&mut self, obj_idx: usize, nested_idx: usize, t: f64) {
        let (judgement, has_windows) = self.judgement_for(obj_idx, Some(nested_idx));
        let label = nested_label(&self.objects[obj_idx], nested_idx);
        self.set_nested_result(obj_idx, nested_idx, judgement.max_result, t, judgement, has_windows, &label);
    }

    fn nested_miss_forcefully(&mut self, obj_idx: usize, nested_idx: usize, t: f64) {
        let (judgement, has_windows) = self.judgement_for(obj_idx, Some(nested_idx));
        let label = nested_label(&self.objects[obj_idx], nested_idx);
        self.set_nested_result(obj_idx, nested_idx, judgement.min_result(), t, judgement, has_windows, &label);
    }

    fn spinner_update(&mut self, obj_idx: usize, t: f64) {
        let (start, end) = (self.objects[obj_idx].start_time, self.objects[obj_idx].end_time);

        // Take the spinner runtime out to avoid borrow conflicts.
        let mut rt = self.runtime[obj_idx]
            .spinner
            .take()
            .expect("spinner runtime");

        // DrawableSpinner.Update: set tracker state.
        let spinnable = start <= t && t < end;
        let any_pressed = self.pressed.contains(&Action::Left) || self.pressed.contains(&Action::Right);
        rt.tracking = spinnable && !rt.judged && any_pressed;

        // RotationTracker.Update.
        let angle = -((self.cursor.x - OBJECT_RADIUS_CENTER.0).atan2(self.cursor.y - OBJECT_RADIUS_CENTER.1))
            .to_degrees();
        let delta = match rt.last_angle {
            Some(last) => {
                let mut d = angle - last;
                if d > 180.0 {
                    d -= 360.0;
                }
                if d < -180.0 {
                    d += 360.0;
                }
                d
            }
            None => 0.0,
        };

        if rt.tracking && spinnable {
            // AddRotation: multiply by the true gameplay rate.
            rt.report_delta(t, delta * self.rate as f32);
        }

        rt.last_angle = Some(angle);

        // UpdateAfterChildren: CheckForResult (final judgement at end),
        // then updateBonusScore (tick awarding per completed spin).
        if !rt.judged && t >= start && t >= end {
            // Trigger a miss result for remaining ticks.
            let tick_count = spinner_tick_count(&self.objects[obj_idx]);
            while rt.next_tick < tick_count {
                let (judgement, has_windows) = self.judgement_for(obj_idx, Some(rt.next_tick));
                let idx = rt.next_tick;
                self.apply_score(obj_idx, Some(idx), judgement, judgement.min_result(), t, 0.0, has_windows, "stick");
                rt.next_tick += 1;
            }

            let judgement = Judgement { max_result: HitResult::Great };
            let progress = spinner_progress_value(&self.objects[obj_idx], &rt);
            let result = if progress >= 1.0 {
                HitResult::Great
            } else if progress > 0.9 {
                HitResult::Ok
            } else if progress > 0.75 {
                HitResult::Meh
            } else {
                HitResult::Miss
            };

            rt.judged = true;
            rt.result = Some(result);
            self.runtime[obj_idx].judged = true;
            self.runtime[obj_idx].result = Some(result);
            self.apply_score(obj_idx, None, judgement, result, t, 0.0, false, "spinner");
        }

        // updateBonusScore
        let spins = (rt.total_rotation() / 360.0) as i32;
        if spins < rt.completed_full_spins {
            rt.completed_full_spins = spins;
        } else {
            let tick_count = spinner_tick_count(&self.objects[obj_idx]);
            while rt.completed_full_spins != spins {
                if rt.next_tick >= tick_count {
                    // Hit the spin limit - no more ticks. lazer plays
                    // `maxBonusSample` here (a sound with NO score event);
                    // emit a timeline marker so renderers can sound it too.
                    self.timeline.push(TimelineEntry {
                        time: t,
                        object_index: obj_idx,
                        label: "smax".to_string(),
                        result: HitResult::IgnoreHit,
                        combo: self.score.combo,
                        score: self.score.total_score(),
                        classic_score: self.score.classic_display_score(),
                        accuracy: self.score.accuracy(),
                        unstable_rate: self.score.unstable_rate(),
                        time_offset: 0.0,
                    });
                    rt.completed_full_spins += 1;
                    continue;
                }

                let (judgement, has_windows) = self.judgement_for(obj_idx, Some(rt.next_tick));
                let idx = rt.next_tick;
                self.apply_score(obj_idx, Some(idx), judgement, judgement.max_result, t, 0.0, has_windows, "stick");
                rt.next_tick += 1;
                rt.completed_full_spins += 1;
            }
        }

        self.runtime[obj_idx].spinner = Some(rt);
    }



    // ------------------------------------------------------------------
    // Slider tracking
    // ------------------------------------------------------------------

    fn update_tracking(&mut self, obj_idx: usize, t: f64) {
        let was_tracking = self.runtime[obj_idx].slider.tracking;

        let head_hit_action = self.runtime[obj_idx].slider.head_hit_action;

        if head_hit_action.is_none() {
            self.runtime[obj_idx].slider.time_to_accept_any_key_after = None;
        }

        if let Some(head_action) = head_hit_action {
            if self.runtime[obj_idx].slider.time_to_accept_any_key_after.is_none() {
                let other = if head_action == Action::Right { Action::Left } else { Action::Right };
                if !self.runtime[obj_idx].slider.last_pressed_actions.contains(&other) {
                    self.runtime[obj_idx].slider.time_to_accept_any_key_after = Some(t);
                }
            }
        }

        // Sample pressed actions into last_pressed_actions and compute validity.
        let last = self.runtime[obj_idx].slider.last_pressed_actions.clone();
        let mut valid_tracking_action = false;
        let mut new_last = Vec::new();
        for &action in &self.pressed {
            if self.is_valid_tracking_action(obj_idx, t, action, head_hit_action) {
                valid_tracking_action = true;
            }
            new_last.push(action);
        }
        self.runtime[obj_idx].slider.last_pressed_actions = new_last;
        let _ = last;

        let all_judged = self.slider_all_judged(obj_idx);
        let end_time = self.objects[obj_idx].end_time;
        let valid_position = self.is_mouse_in_follow_area(obj_idx, t, was_tracking);

        self.runtime[obj_idx].slider.tracking =
            (!all_judged || t <= end_time) && valid_position && valid_tracking_action;
    }

    fn update_tracking_with(&mut self, obj_idx: usize, t: f64, valid_position: bool) {
        // Used by PostProcessHeadJudgement: tracking = (!allJudged || t <= end) && valid && anyValidAction
        let head_hit_action = self.runtime[obj_idx].slider.head_hit_action;
        let mut valid_tracking_action = false;
        let mut new_last = Vec::new();
        for &action in &self.pressed {
            if self.is_valid_tracking_action(obj_idx, t, action, head_hit_action) {
                valid_tracking_action = true;
            }
            new_last.push(action);
        }
        self.runtime[obj_idx].slider.last_pressed_actions = new_last;

        let all_judged = self.slider_all_judged(obj_idx);
        let end_time = self.objects[obj_idx].end_time;
        self.runtime[obj_idx].slider.tracking =
            (!all_judged || t <= end_time) && valid_position && valid_tracking_action;
    }

    fn is_valid_tracking_action(&self, obj_idx: usize, t: f64, action: Action, hit_action: Option<Action>) -> bool {
        if let Some(hit_action) = hit_action {
            let accept_any_after = self.runtime[obj_idx].slider.time_to_accept_any_key_after;
            if accept_any_after.is_none() || t <= accept_any_after.unwrap() {
                return action == hit_action;
            }
        }
        matches!(action, Action::Left | Action::Right)
    }

    fn is_mouse_in_follow_area(&self, obj_idx: usize, t: f64, expanded: bool) -> bool {
        let obj = &self.objects[obj_idx];
        let path_progress_pos = match &obj.kind {
            ProcKind::Slider { duration, .. } => {
                let follow_progress = ((t - obj.start_time) / duration).clamp(0.0, 1.0);
                path_position_at(obj, follow_progress)
            }
            _ => return false,
        };

        let radius = if expanded {
            obj.radius * FOLLOW_AREA
        } else {
            obj.radius
        };

        let mouse_position_in_slider = self.cursor - obj.stacked_position();
        (mouse_position_in_slider - path_progress_pos).length_squared() <= radius * radius
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn slider_all_judged(&self, obj_idx: usize) -> bool {
        self.runtime[obj_idx].judged && self.runtime[obj_idx].nested_judged.iter().all(|&j| j)
    }

    fn top_level_all_judged(&self, obj_idx: usize) -> bool {
        match &self.objects[obj_idx].kind {
            ProcKind::Slider { .. } => self.slider_all_judged(obj_idx),
            _ => self.runtime[obj_idx].judged,
        }
    }

    fn nested_is_judged(&self, obj_idx: usize, nested_idx: usize) -> bool {
        self.runtime[obj_idx].nested_judged[nested_idx]
    }

    fn find_nested(&self, obj_idx: usize, kind: NestedKind) -> Option<usize> {
        nested_of(&self.objects[obj_idx])
            .iter()
            .position(|n| n.kind == kind)
    }

    fn set_nested_result(
        &mut self,
        obj_idx: usize,
        nested_idx: usize,
        result: HitResult,
        t: f64,
        judgement: Judgement,
        has_windows: bool,
        label: &str,
    ) {
        self.runtime[obj_idx].nested_judged[nested_idx] = true;
        self.runtime[obj_idx].nested_results[nested_idx] = Some(result);
        let nested_time = nested_of(&self.objects[obj_idx])[nested_idx].time;
        let time_offset = t - nested_time;
        self.apply_score(obj_idx, Some(nested_idx), judgement, result, t, time_offset, has_windows, label);
    }

    fn apply_score(
        &mut self,
        obj_idx: usize,
        nested_idx: Option<usize>,
        judgement: Judgement,
        result: HitResult,
        t: f64,
        time_offset: f64,
        has_windows: bool,
        label: &str,
    ) {
        // TimeOffset for hit events: min(raw - end, MaximumJudgementOffset).
        let end_time = match nested_idx {
            None => self.objects[obj_idx].end_time,
            Some(n) => match &self.objects[obj_idx].kind {
                ProcKind::Slider { nested, .. } => nested[n].time,
                ProcKind::Spinner { ticks, .. } => ticks[n],
                ProcKind::Circle => self.objects[obj_idx].end_time,
            },
        };
        let max_judgement_offset = if has_windows { MISS_WINDOW } else { 0.0 };
        let event_offset = (t - end_time).min(max_judgement_offset);

        self.score
            .apply_result(judgement, result, t, event_offset, has_windows, self.rate);

        // `JudgementResult.TimeAbsolute`: min(RawTime, GetEndTime() +
        // MaximumJudgementOffset). `event_offset` is already that clamped
        // delta, so end + event_offset reproduces lazer's recorded
        // timestamp (late frame-evaluated results stamp at the clamp).
        // Spinner ticks are exempt: their MaximumJudgementOffset is the
        // whole spinner duration (the clamp never binds there), and they
        // fire at the actual full-rotation moment, not the grid.
        let entry = TimelineEntry {
            time: if label == "stick" || label == "smax" { t } else { end_time + event_offset },
            object_index: obj_idx,
            label: label.to_string(),
            result,
            combo: self.score.combo,
            score: self.score.total_score(),
            classic_score: self.score.classic_display_score(),
            accuracy: self.score.accuracy(),
            unstable_rate: self.score.unstable_rate(),
            time_offset,
        };
        self.timeline.push(entry);
    }

    /// `HealthProcessor` pass (`DrainingHealthProcessor` +
    /// `OsuHealthProcessor`): solves the drain rate off a perfect-play
    /// increase list (mirroring `SimulateAutoplay`'s enumeration -
    /// nested first, then the object itself, max results, bonus results
    /// excluded from the collection), then walks the live timeline
    /// applying the health-increase table with the combo-end bonus and
    /// the fail condition.
    fn build_health(&self) -> crate::health::HealthInfo {
        use crate::health as h;

        let drain_start = self.objects.first().map(|o| o.start_time).unwrap_or(0.0);
        let drain_end = self.objects.last().map(|o| o.end_time).unwrap_or(0.0);

        // No-drain periods: `PeriodTracker` over `beatmap.Breaks` - each
        // break widens to the surrounding object gap
        // (`[last object end <= break.start, first object start >= break.end]`).
        let mut no_drain: Vec<(f64, f64)> = Vec::new();
        for &(b0, b1) in &self.breaks {
            let p0 = self
                .objects
                .iter()
                .map(|o| o.end_time)
                .filter(|&t| t <= b0)
                .fold(f64::NEG_INFINITY, f64::max);
            let p1 = self
                .objects
                .iter()
                .map(|o| o.start_time)
                .filter(|&t| t >= b1)
                .fold(f64::INFINITY, f64::min);
            no_drain.push((p0, p1));
        }

        let mut info = crate::health::HealthInfo {
            drain_start,
            drain_end,
            no_drain,
            ..Default::default()
        };

        // Perfect-play increase list for the solver. `JudgementProcessor
        // .SimulateAutoplay` enumerates nested objects BEFORE their parent,
        // applying each judgement's MaxResult; `DrainingHealthProcessor
        // .ApplyResultInternal` collects every non-bonus result's health
        // increase at `GetEndTime()` (TimeOffset 0 in the simulation).
        let mut sim: Vec<(f64, f64)> = Vec::new();
        for (i, obj) in self.objects.iter().enumerate() {
            match &obj.kind {
                ProcKind::Slider { nested, .. } => {
                    for n in nested {
                        let result = match n.kind {
                            NestedKind::Head => {
                                if self.classic {
                                    HitResult::LargeTickHit
                                } else {
                                    HitResult::Great
                                }
                            }
                            NestedKind::Tick | NestedKind::Repeat => HitResult::LargeTickHit,
                            NestedKind::Tail => {
                                if self.classic {
                                    HitResult::SmallTickHit
                                } else {
                                    HitResult::SliderTailHit
                                }
                            }
                        };
                        let is_tick = n.kind == NestedKind::Tick;
                        if !h::is_bonus(result) {
                            sim.push((n.time, h::increase_for(result, is_tick, self.hp_drain)));
                        }
                    }
                    // The slider's own (body) judgement closes the object.
                    let (judgement, _) = self.judgement_for(i, None);
                    if !h::is_bonus(judgement.max_result) {
                        sim.push((obj.end_time, h::increase_for(judgement.max_result, false, self.hp_drain)));
                    }
                }
                ProcKind::Spinner { .. } => {
                    // Spinner bonus ticks are bonus results (excluded from
                    // the solver collection); the spinner itself is judged
                    // Great at its end.
                    sim.push((obj.end_time, h::increase_for(HitResult::Great, false, self.hp_drain)));
                }
                ProcKind::Circle => {
                    sim.push((obj.start_time, h::increase_for(HitResult::Great, false, self.hp_drain)));
                }
            }
        }

        info.drain_rate = h::compute_drain_rate(
            &sim,
            drain_start,
            &self.breaks,
            h::target_minimum_health(self.hp_drain),
        );

        // Live playthrough: drain integrates between judgements (clamped
        // at 0 like the health bindable), then each increase applies
        // through `OsuHealthProcessor.GetHealthIncreaseFor` - the running
        // `ComboResult` degrades per result type on every object carrying
        // combo info, resets on NewCombo objects, and the bonus applies on
        // top-level (`LastInCombo`) hit judgements.
        let mut events: Vec<(f64, f64)> = Vec::with_capacity(self.timeline.len());
        let mut hp = 1.0f64;
        let mut prev_t = drain_start;
        let mut combo_result = 2u8; // ComboResult: None=0 < Good=1 < Perfect=2
        let mut failed = false;

        for e in &self.timeline {
            hp = (hp - info.drain_rate * h::drain_span(&info, prev_t, e.time)).max(0.0);
            prev_t = e.time;

            let is_tick = e.label.starts_with("tick");
            let mut amount = h::increase_for(e.result, is_tick, self.hp_drain);

            if e.label != "smax" {
                let obj = &self.objects[e.object_index];
                if obj.new_combo {
                    combo_result = 2;
                }
                match e.result {
                    HitResult::LargeTickMiss | HitResult::Ok => combo_result = combo_result.min(1),
                    HitResult::Meh | HitResult::Miss => combo_result = combo_result.min(0),
                    _ => {}
                }
                // The slider-tail miss is a special judgement that degrades
                // to Good.
                if e.label == "tail" && !hit_result_ext::is_hit(e.result) {
                    combo_result = combo_result.min(1);
                }
                // `combo.LastInCombo` is only ever set on TOP-LEVEL objects
                // (`UpdateComboInformation` marks the previous top-level
                // object; nested slider pieces keep the default false), so
                // the bonus fires on circle/slider-body/spinner judgements
                // only.
                if matches!(e.label.as_str(), "circle" | "spinner")
                    && self.last_in_combo.get(e.object_index).copied().unwrap_or(false)
                    && hit_result_ext::is_hit(e.result)
                {
                    amount += match combo_result {
                        2 => 0.07,
                        1 => 0.05,
                        _ => 0.03,
                    };
                }
            }

            hp = (hp + amount).clamp(0.0, 1.0);

            // `CheckDefaultFailCondition` (bonus results exempt).
            if hp <= 0.0 && !h::is_bonus(e.result) && e.result != HitResult::IgnoreHit {
                failed = true;
            }

            events.push((e.time, hp));
        }

        info.events = events;
        info.failed = failed;
        info
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ClickAction {
    Ignore,
    Shake,
    Hit,
}

fn head_index_of(obj: &ProcObject) -> Option<usize> {
    match &obj.kind {
        ProcKind::Slider { nested, .. } => nested.iter().position(|n| n.kind == NestedKind::Head),
        _ => None,
    }
}

fn head_start(obj: &ProcObject) -> f64 {
    head_index_of(obj)
        .and_then(|i| nested_of(obj).get(i).map(|n| n.time))
        .unwrap_or(obj.start_time)
}

fn nested_of(obj: &ProcObject) -> &[crate::process::NestedObj] {
    match &obj.kind {
        ProcKind::Slider { nested, .. } => nested,
        _ => &[],
    }
}

fn nested_label(obj: &ProcObject, nested_idx: usize) -> String {
    let nested = nested_of(obj);
    match nested[nested_idx].kind {
        NestedKind::Head => "head".to_string(),
        NestedKind::Tail => "tail".to_string(),
        NestedKind::Tick => format!("tick{}", nested_idx),
        NestedKind::Repeat => format!("repeat{}", nested_idx),
    }
}

fn spinner_progress_value(obj: &ProcObject, rt: &SpinnerRuntime) -> f32 {
    match &obj.kind {
        ProcKind::Spinner { spins_required, .. } => {
            if *spins_required == 0 {
                1.0
            } else {
                (rt.total_rotation() / 360.0 / *spins_required as f32).clamp(0.0, 1.0)
            }
        }
        _ => unreachable!(),
    }
}

fn spinner_tick_count(obj: &ProcObject) -> usize {
    match &obj.kind {
        ProcKind::Spinner { ticks, .. } => ticks.len(),
        _ => 0,
    }
}
