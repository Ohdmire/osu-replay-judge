//! Post-decode beatmap processing: difficulty mods, object defaults,
//! stacking and nested object creation - mirroring lazer's
//! `WorkingBeatmap.GetPlayableBeatmap` pipeline for the osu! ruleset.

use crate::beatmap::{
    calculate_scale_from_circle_size, difficulty_range, difficulty_range_int, Beatmap, Difficulty,
    RawHitObject,
};
use crate::path::{PathControlPoint, SliderPath};
use crate::vec2::Vec2;

pub const OBJECT_RADIUS: f32 = 64.0;
pub const STACK_DISTANCE: f32 = 3.0;
pub const BASE_SCORING_DISTANCE: f64 = 100.0;
pub const TAIL_LENIENCY: f64 = -36.0;

/// `OsuPlayfield.BASE_SIZE.Y`: HR reflects objects along the playfield
/// (`OsuHitObjectGenerationUtils.ReflectVerticallyAlongPlayfield`).
const PLAYFIELD_BASE_HEIGHT: f32 = 384.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedKind {
    Head,
    Tick,
    Repeat,
    Tail,
}

#[derive(Clone, Debug)]
pub struct NestedObj {
    pub kind: NestedKind,
    pub time: f64,
    pub span_index: usize,
    pub span_start_time: f64,
    pub path_progress: f64,
}

pub enum ProcKind {
    Circle,
    Slider {
        path: SliderPath,
        nested: Vec<NestedObj>,
        velocity: f64,
        span_count: usize,
        span_duration: f64,
        duration: f64,
    },
    Spinner {
        spins_required: usize,
        maximum_bonus_spins: usize,
        /// Tick times + whether bonus (LargeBonus); len = total spins.
        ticks: Vec<f64>,
    },
}

pub struct ProcObject {
    pub start_time: f64,
    pub end_time: f64,
    pub position: Vec2,
    pub end_position: Vec2,
    pub stack_height: i32,
    pub scale: f32,
    pub radius: f32,
    pub time_preempt: f64,
    pub kind: ProcKind,
    /// Combo chain (`OsuHitObject.UpdateComboInformation`).
    pub new_combo: bool,
    pub index_in_current_combo: u32,
    /// 1-based combo colour index (colour = `(index-1) % colour_count`).
    pub combo_index_with_offsets: u32,
}

impl ProcObject {
    pub fn is_spinner(&self) -> bool {
        matches!(self.kind, ProcKind::Spinner { .. })
    }

    pub fn is_slider(&self) -> bool {
        matches!(self.kind, ProcKind::Slider { .. })
    }

    pub fn is_circle(&self) -> bool {
        matches!(self.kind, ProcKind::Circle)
    }

    pub fn stack_offset(&self) -> Vec2 {
        Vec2::new(self.stack_height as f32 * self.scale * -6.4, self.stack_height as f32 * self.scale * -6.4)
    }

    pub fn stacked_position(&self) -> Vec2 {
        self.position + self.stack_offset()
    }
}

pub struct ProcessedBeatmap {
    pub difficulty: Difficulty,
    pub objects: Vec<ProcObject>,
}

/// Applies difficulty mods to a copy of the difficulty
/// (`IApplicableToDifficulty` implementations used by legacy mod conversion).
pub fn apply_difficulty_mods(mut difficulty: Difficulty, hard_rock: bool, easy: bool) -> Difficulty {
    // ModHardRock.ApplyToDifficulty + OsuModHardRock override.
    if hard_rock {
        difficulty.hp = (difficulty.hp * 1.4).min(10.0);
        difficulty.od = (difficulty.od * 1.4).min(10.0);
        difficulty.cs = (difficulty.cs * 1.3).min(10.0);
        difficulty.ar = (difficulty.ar * 1.4).min(10.0);
    }

    // ModEasy.ApplyToDifficulty (base: CS/AR/HP) + OsuModEasy override (OD).
    if easy {
        difficulty.cs *= 0.5;
        difficulty.ar *= 0.5;
        difficulty.hp *= 0.5;
        difficulty.od *= 0.5;
    }

    difficulty
}

pub fn process(
    beatmap: &Beatmap,
    difficulty: Difficulty,
    classic_slider_behaviour: bool,
    hard_rock: bool,
) -> ProcessedBeatmap {
    // Stage 1: object defaults (ApplyDefaults) - preempt, scale, slider
    // velocity/ticks, spinner requirements.
    //
    // HR's vertical reflection (`OsuModHardRock.ApplyToHitObject`) runs in
    // lazer after ApplyDefaults and before stacking; here it is folded into
    // the same pass, which is numerically equivalent: reflection preserves
    // path length (so velocity/tick times/duration are unchanged) and no
    // default depends on position. Stacking (stage 2) still sees mirrored
    // positions, matching lazer.
    let mut objects: Vec<ProcObject> = Vec::with_capacity(beatmap.objects.len());

    for obj in &beatmap.objects {
        let time_preempt = difficulty_range_int(difficulty.ar as f64, 1800.0, 1200.0, 450.0) as f64;
        let scale = calculate_scale_from_circle_size(difficulty.cs);
        let radius = OBJECT_RADIUS * scale;

        match &obj.raw {
            RawHitObject::Circle { pos } => {
                // ReflectVerticallyAlongPlayfield (spinners are centre-fixed,
                // so the reflection is a no-op there).
                let pos = if hard_rock {
                    Vec2::new(pos.x, PLAYFIELD_BASE_HEIGHT - pos.y)
                } else {
                    *pos
                };
                objects.push(ProcObject {
                    start_time: obj.start_time,
                    end_time: obj.start_time,
                    position: pos,
                    end_position: pos,
                    stack_height: 0,
                    scale,
                    radius,
                    time_preempt,
                    kind: ProcKind::Circle,
                    new_combo: obj.new_combo,
                    index_in_current_combo: 0,
                    combo_index_with_offsets: 0,
                });
            }
            RawHitObject::Slider { pos, path, repeat_count } => {
                // HR: reflect head Y (`Position = (X, BASE_SIZE.Y - Y)`) and
                // negate slider control-point Ys (`reflectControlPoint`),
                // rebuilding the path — `modifySlider` in lazer.
                let pos = if hard_rock {
                    Vec2::new(pos.x, PLAYFIELD_BASE_HEIGHT - pos.y)
                } else {
                    *pos
                };
                let control_points = if hard_rock {
                    path.control_points
                        .iter()
                        .map(|cp| PathControlPoint {
                            position: Vec2::new(cp.position.x, -cp.position.y),
                            kind: cp.kind,
                        })
                        .collect()
                } else {
                    path.control_points.clone()
                };

                let timing_point = beatmap.timing_point_at(obj.start_time);

                // GetPrecisionAdjustedBeatLength (osu branch).
                let slider_velocity_as_beat_length = -100.0 / obj.slider_velocity_multiplier;
                let bpm_multiplier = if slider_velocity_as_beat_length < 0.0 {
                    let clamped = (slider_velocity_as_beat_length.abs() as f32).clamp(10.0, 1000.0);
                    clamped as f64 / 100.0
                } else {
                    1.0
                };
                let adjusted_beat_length = timing_point.beat_length * bpm_multiplier;

                // BASE_SCORING_DISTANCE and SliderMultiplier are floats in C#:
                // the product rounds to f32 before the double division.
                let velocity = (BASE_SCORING_DISTANCE as f32 * difficulty.slider_multiplier) as f64 / adjusted_beat_length;
                // Intentionally not BASE_SCORING_DISTANCE * SliderMultiplier (stable float compat).
                let scoring_distance = velocity * timing_point.beat_length;

                let generate_ticks = beatmap.difficulty_point_at(obj.start_time).generate_ticks;
                let tick_distance = if generate_ticks {
                    scoring_distance / difficulty.slider_tick_rate as f64
                } else {
                    f64::INFINITY
                };

                let path = SliderPath::new(
                    control_points,
                    path.expected_distance,
                );
                let distance = path.distance();
                let span_count = repeat_count + 1;
                let duration = span_count as f64 * distance / velocity;
                let span_duration = duration / span_count as f64;

                let nested = generate_slider_nested(obj.start_time, span_duration, velocity, tick_distance, distance, span_count);

                // EndPosition = Position + CurvePositionAt(1): mirrors for even spans.
                let span_count_f = span_count as f64;
                let mut end_p = (1.0 * span_count_f) % 1.0;
                let end_span = (1.0 * span_count_f) as i32;
                if end_span % 2 == 1 {
                    end_p = 1.0 - end_p;
                }
                let end_position = pos + path.position_at(end_p);

                objects.push(ProcObject {
                    start_time: obj.start_time,
                    end_time: obj.start_time + duration,
                    position: pos,
                    end_position,
                    stack_height: 0,
                    scale,
                    radius,
                    time_preempt,
                    kind: ProcKind::Slider { path, nested, velocity, span_count, span_duration, duration },
                    new_combo: obj.new_combo,
                    index_in_current_combo: 0,
                    combo_index_with_offsets: 0,
                });
            }
            RawHitObject::Spinner { end_time } => {
                let duration = (*end_time - obj.start_time).max(0.0);

                let min_rps = difficulty_range(difficulty.od as f64, 90.0, 150.0, 225.0) / 60.0;
                let max_rps = difficulty_range(difficulty.od as f64, 250.0, 380.0, 430.0) / 60.0;
                let seconds_duration = duration / 1000.0;
                const DURATION_ERROR: f64 = 0.0001;

                let spins_required = (min_rps * seconds_duration + DURATION_ERROR) as i32 as usize;
                let maximum_bonus_spins = (((max_rps * seconds_duration + DURATION_ERROR) as i32
                    - spins_required as i32
                    - 2)
                    .max(0)) as usize;

                const BONUS_SPINS_GAP: usize = 2;
                let total_spins = maximum_bonus_spins + spins_required + BONUS_SPINS_GAP;

                let mut ticks = Vec::with_capacity(total_spins);
                for i in 0..total_spins {
                    let t = obj.start_time + ((i + 1) as f32 / total_spins as f32) as f64 * duration;
                    ticks.push(t);
                }

                objects.push(ProcObject {
                    start_time: obj.start_time,
                    end_time: obj.start_time + duration,
                    position: Vec2::new(256.0, 192.0),
                    end_position: Vec2::new(256.0, 192.0),
                    stack_height: 0,
                    scale,
                    radius,
                    time_preempt,
                    kind: ProcKind::Spinner { spins_required, maximum_bonus_spins, ticks },
                    new_combo: obj.new_combo,
                    index_in_current_combo: 0,
                    combo_index_with_offsets: 0,
                });
            }
        }
    }

    // Objects arrive sorted by start time from the decoder.
    // Stage 2: stacking (PostProcess).
    apply_stacking(&mut objects, beatmap.version, beatmap.stack_leniency);

    // Stage 3: combo chain (`OsuBeatmapProcessor.UpdateComboColours` ->
    // `UpdateComboInformation` per object, in order).
    {
        let mut index_with_offsets = 0u32;
        let mut in_current_combo = 0u32;
        let mut prev_is_spinner = false;

        for (i, obj) in objects.iter_mut().enumerate() {
            let is_spinner = obj.is_spinner();
            let starts_new = !is_spinner && (obj.new_combo || i == 0 || prev_is_spinner);

            if starts_new {
                in_current_combo = 0;
                let combo_offset = beatmap.objects[i].combo_offset;
                index_with_offsets += combo_offset + 1;
            }

            obj.index_in_current_combo = in_current_combo;
            obj.combo_index_with_offsets = index_with_offsets;
            in_current_combo += 1;
            prev_is_spinner = is_spinner;
        }
    }

    let _ = classic_slider_behaviour;

    ProcessedBeatmap { difficulty, objects }
}

/// Port of `SliderEventGenerator.Generate` (skipping LegacyLastTick).
fn generate_slider_nested(
    start_time: f64,
    span_duration: f64,
    velocity: f64,
    tick_distance: f64,
    total_distance: f64,
    span_count: usize,
) -> Vec<NestedObj> {
    const MAX_LENGTH: f64 = 100000.0;

    let length = MAX_LENGTH.min(total_distance);
    let tick_distance = tick_distance.clamp(0.0, length);
    let min_distance_from_end = velocity * 10.0;

    let mut events: Vec<NestedObj> = Vec::new();

    events.push(NestedObj {
        kind: NestedKind::Head,
        time: start_time,
        span_index: 0,
        span_start_time: start_time,
        path_progress: 0.0,
    });

    for span in 0..span_count {
        let span_start_time = start_time + span as f64 * span_duration;
        let reversed = span % 2 == 1;

        if tick_distance != 0.0 {
            let mut ticks: Vec<NestedObj> = Vec::new();

            let mut d = tick_distance;
            while d <= length {
                if d >= length - min_distance_from_end {
                    break;
                }

                let path_progress = d / length;
                let time_progress = if reversed { 1.0 - path_progress } else { path_progress };

                ticks.push(NestedObj {
                    kind: NestedKind::Tick,
                    time: span_start_time + time_progress * span_duration,
                    span_index: span,
                    span_start_time,
                    path_progress,
                });

                d += tick_distance;
            }

            if reversed {
                ticks.reverse();
            }

            events.extend(ticks);
        }

        if span < span_count - 1 {
            events.push(NestedObj {
                kind: NestedKind::Repeat,
                time: span_start_time + span_duration,
                span_index: span,
                span_start_time: start_time + span as f64 * span_duration,
                path_progress: ((span + 1) % 2) as f64,
            });
        }
    }

    let total_duration = span_count as f64 * span_duration;

    events.push(NestedObj {
        kind: NestedKind::Tail,
        time: start_time + total_duration,
        span_index: span_count - 1,
        span_start_time: start_time + (span_count - 1) as f64 * span_duration,
        path_progress: (span_count % 2) as f64,
    });

    // Nested objects are sorted by start time (HitObject.ApplyDefaults).
    events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());

    events
}

/// Port of `OsuBeatmapProcessor.ApplyStacking`.
fn apply_stacking(objects: &mut Vec<ProcObject>, version: i32, stack_leniency: f32) {
    if objects.is_empty() {
        return;
    }

    for obj in objects.iter_mut() {
        obj.stack_height = 0;
    }

    if version >= 6 {
        let end = objects.len().saturating_sub(1);
        apply_stacking_new(objects, 0, end, stack_leniency);
    } else {
        apply_stacking_old(objects, stack_leniency);
    }
}

fn stack_threshold(obj: &ProcObject, stack_leniency: f32) -> f32 {
    obj.time_preempt as i32 as f32 * stack_leniency
}

fn apply_stacking_new(objects: &mut [ProcObject], start_index: usize, end_index: usize, stack_leniency: f32) {
    let mut extended_end_index = end_index;

    if end_index < objects.len() - 1 {
        // Extend the end index to include objects they are stacked on.
        for i in (start_index..=end_index).rev() {
            let mut stack_base_index = i;

            let mut n = stack_base_index + 1;
            while n < objects.len() {
                if objects[stack_base_index].is_spinner() {
                    break;
                }
                if objects[n].is_spinner() {
                    n += 1;
                    continue;
                }

                let end_time = objects[stack_base_index].end_time;
                let threshold = stack_threshold(&objects[n], stack_leniency) as f64;

                if objects[n].start_time - end_time > threshold {
                    break;
                }

                if Vec2::distance(objects[stack_base_index].position, objects[n].position) < STACK_DISTANCE
                    || (objects[stack_base_index].is_slider()
                        && Vec2::distance(objects[stack_base_index].end_position, objects[n].position)
                            < STACK_DISTANCE)
                {
                    stack_base_index = n;
                    // HitObjects after the specified update range haven't been reset yet.
                    objects[n].stack_height = 0;
                }

                n += 1;
            }

            if stack_base_index > extended_end_index {
                extended_end_index = stack_base_index;
                if extended_end_index == objects.len() - 1 {
                    break;
                }
            }
        }
    }

    // Reverse pass for stack calculation.
    let mut extended_start_index = start_index;

    for i in (start_index + 1..=extended_end_index).rev() {
        if objects[i].stack_height != 0 || objects[i].is_spinner() {
            continue;
        }

        let threshold = stack_threshold(&objects[i], stack_leniency) as f64;
        let is_circle = objects[i].is_circle();

        if is_circle {
            let mut object_i = i;
            let mut n = i;
            while n > 0 {
                n -= 1;
                if objects[n].is_spinner() {
                    continue;
                }

                let end_time = objects[n].end_time;

                // Truncation to integer is required to match stable.
                if objects[object_i].start_time as i64 - end_time as i64 > threshold as i64 {
                    // We are no longer within stacking range of the previous object.
                    break;
                }

                // HitObjects before the specified update range haven't been reset yet.
                if n < extended_start_index {
                    objects[n].stack_height = 0;
                    extended_start_index = n;
                }

                /* Special case: circles under the last slider get negative stacks. */
                if objects[n].is_slider()
                    && Vec2::distance(objects[n].end_position, objects[object_i].position) < STACK_DISTANCE
                {
                    let offset = objects[object_i].stack_height - objects[n].stack_height + 1;

                    for j in n + 1..=i {
                        if Vec2::distance(objects[n].end_position, objects[j].position) < STACK_DISTANCE {
                            objects[j].stack_height -= offset;
                        }
                    }

                    // We have hit a slider. Restart calculation using this as the new base.
                    break;
                }

                if Vec2::distance(objects[n].position, objects[object_i].position) < STACK_DISTANCE {
                    // Keep processing as if there are no sliders.
                    let height = objects[object_i].stack_height + 1;
                    objects[n].stack_height = height;
                    // objectI = objectN
                    object_i = n;
                }
            }
        } else if objects[i].is_slider() {
            let mut object_i = i;
            let mut n = i;
            while n > start_index {
                n -= 1;
                if objects[n].is_spinner() {
                    continue;
                }

                if objects[object_i].start_time - objects[n].start_time > threshold {
                    break;
                }

                if Vec2::distance(objects[n].end_position, objects[object_i].position) < STACK_DISTANCE {
                    let height = objects[object_i].stack_height + 1;
                    objects[n].stack_height = height;
                    // objectI = objectN
                    object_i = n;
                }
            }
        }
    }
}

/// Port of `OsuBeatmapProcessor.applyStackingOld` (beatmap version < 6).
fn apply_stacking_old(objects: &mut [ProcObject], stack_leniency: f32) {
    for i in 0..objects.len() {
        if objects[i].stack_height != 0 && !objects[i].is_slider() {
            continue;
        }

        let mut start_time = objects[i].end_time;
        let mut slider_stack = 0;

        let position = objects[i].position;
        let position2 = if objects[i].is_slider() {
            objects[i].end_position
        } else {
            objects[i].position
        };

        let threshold = stack_threshold(&objects[i], stack_leniency) as f64;

        for j in i + 1..objects.len() {
            if objects[j].start_time - threshold > start_time {
                break;
            }

            if Vec2::distance(objects[j].position, position) < STACK_DISTANCE {
                objects[i].stack_height += 1;
                start_time = objects[j].start_time;
            } else if Vec2::distance(objects[j].position, position2) < STACK_DISTANCE {
                // Case for sliders - bump notes down and right.
                slider_stack += 1;
                objects[j].stack_height -= slider_stack;
                start_time = objects[j].start_time;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beatmap::{Beatmap, Difficulty, DifficultyPoint, ParsedObject, TimingPoint};
    use crate::path::{PathControlPoint, PathType};

    fn test_beatmap() -> Beatmap {
        Beatmap {
            version: 14,
            stack_leniency: 0.7,
            difficulty: Difficulty::default(),
            timing_points: vec![TimingPoint { time: 0.0, beat_length: 500.0 }],
            difficulty_points: vec![DifficultyPoint {
                time: 0.0,
                slider_velocity: 1.0,
                generate_ticks: true,
            }],
            objects: vec![
                ParsedObject {
                    start_time: 1000.0,
                    new_combo: true,
                    raw: RawHitObject::Circle { pos: Vec2::new(200.0, 100.0) },
                    slider_velocity_multiplier: 1.0,
                    combo_offset: 0,
                },
                ParsedObject {
                    start_time: 2000.0,
                    new_combo: true,
                    raw: RawHitObject::Slider {
                        pos: Vec2::new(100.0, 100.0),
                        path: SliderPath::new(
                            vec![
                                PathControlPoint { position: Vec2::ZERO, kind: Some(PathType::LINEAR) },
                                PathControlPoint { position: Vec2::new(100.0, 50.0), kind: None },
                            ],
                            None,
                        ),
                        repeat_count: 0,
                    },
                    slider_velocity_multiplier: 1.0,
                    combo_offset: 0,
                },
            ],
            combo_colours: Vec::new(),
        }
    }

    /// `OsuModHardRock.ApplyToHitObject` -> `ReflectVerticallyAlongPlayfield`:
    /// head `Y -> BASE_SIZE.Y - Y`, slider control points `Y -> -Y` (path
    /// rebuilt), end position follows.
    #[test]
    fn hard_rock_reflects_objects_vertically() {
        let map = test_beatmap();
        let processed = process(&map, map.difficulty, false, true);

        let circle = &processed.objects[0];
        assert_eq!(circle.position, Vec2::new(200.0, 384.0 - 100.0));

        let slider = &processed.objects[1];
        assert_eq!(slider.position, Vec2::new(100.0, 384.0 - 100.0));
        match &slider.kind {
            ProcKind::Slider { path, .. } => {
                assert_eq!(path.control_points[1].position, Vec2::new(100.0, -50.0));
            }
            _ => panic!("expected slider"),
        }
        // Original end (200, 150) reflects to (200, 234).
        assert_eq!(slider.end_position, Vec2::new(200.0, 384.0 - 150.0));
    }

    #[test]
    fn without_hard_rock_positions_are_unchanged() {
        let map = test_beatmap();
        let processed = process(&map, map.difficulty, false, false);

        assert_eq!(processed.objects[0].position, Vec2::new(200.0, 100.0));
        match &processed.objects[1].kind {
            ProcKind::Slider { path, .. } => {
                assert_eq!(path.control_points[1].position, Vec2::new(100.0, 50.0));
            }
            _ => panic!("expected slider"),
        }
    }

    /// `ModEasy` halves CS/AR/HP, `OsuModEasy` additionally halves OD.
    #[test]
    fn easy_halves_all_difficulty_values() {
        let d = apply_difficulty_mods(
            Difficulty { hp: 6.0, cs: 5.0, od: 8.0, ar: 9.0, ..Difficulty::default() },
            false,
            true,
        );
        assert_eq!(d.cs, 2.5);
        assert_eq!(d.ar, 4.5);
        assert_eq!(d.hp, 3.0);
        assert_eq!(d.od, 4.0);
    }
}
