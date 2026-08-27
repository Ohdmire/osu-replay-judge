//! Port of the osu! health processors:
//!
//! - `osu.Game/Rulesets/Scoring/HealthProcessor.cs` - the 0..1 health
//!   bindable, the fail condition (bonus results exempt)
//! - `osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs` - constant
//!   drain between `DrainStartTime` (first object start) and the last
//!   object end, no-drain periods around beatmap breaks, the perfect-play
//!   drain-rate solver (`ComputeDrainRate`) targeting the HP-curve of
//!   minimum health (0.99 / 0.9 / 0.4 at HP 0 / 5 / 10)
//! - `osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs` - the per-result
//!   health increase table (miss amounts scale with the beatmap HP drain)
//!   and the combo-end bonus (`ComboResult` Perfect/Good/None tracking)
//!
//! The live playthrough is reconstructed from the judgement timeline
//! (`Engine::timeline`, already in lazer's applied order with lazer's
//! `TimeAbsolute` stamps): drain integrates linearly between judgements
//! and each increase clamps the same way the framework `BindableDouble`
//! (min 0 max 1) does, so a post-hoc walk reproduces the interleaved
//! frame-drain + apply exactly.

use crate::score::HitResult;

/// Resolved health state of a playthrough (Engine::health).
#[derive(Clone, Debug, Default)]
pub struct HealthInfo {
    /// Solved drain rate (health units per ms) - `DrainRate`.
    pub drain_rate: f64,
    /// First object start time - the `DrainStartTime` the player passes
    /// (`playableBeatmap.HitObjects[0].StartTime`).
    pub drain_start: f64,
    /// Last object end time - `gameplayEndTime`.
    pub drain_end: f64,
    /// No-drain periods around breaks (`PeriodTracker` over
    /// `beatmap.Breaks`): each break widens to
    /// `[last object end <= break.start, first object start >= break.end]`.
    pub no_drain: Vec<(f64, f64)>,
    /// `(time, health after increase)` per timeline judgement, in
    /// timeline order. Health at an arbitrary time interpolates the
    /// drain backwards from the next-latest entry (`health_at`).
    pub events: Vec<(f64, f64)>,
    /// The default fail condition fired at some point (`Health.Value
    /// <= 0` on a non-bonus judgement). Informational - replays of
    /// passing scores never set this.
    pub failed: bool,
}

/// `IBeatmapDifficultyInfo.DifficultyRange` (linear interpolation through
/// `mid` at 5, `min` at 0 and `max` at 10).
pub fn difficulty_range(hp: f64, min: f64, mid: f64, max: f64) -> f64 {
    if hp > 5.0 {
        mid + (max - mid) * (hp - 5.0) / 5.0
    } else if hp < 5.0 {
        mid + (mid - min) * (hp - 5.0) / 5.0
    } else {
        mid
    }
}

/// The minimum-health solver target at the beatmap's HP drain rate
/// (`DrainingHealthProcessor`: 0.99 / 0.9 / 0.4 at HP 0 / 5 / 10).
pub fn target_minimum_health(hp_drain: f64) -> f64 {
    difficulty_range(hp_drain, 0.99, 0.9, 0.4)
}

/// `OsuHealthProcessor.getHealthIncreaseFor`: the health delta of one
/// judgement. `is_slider_tick` distinguishes `SliderTick` nested objects
/// (+0.015) from the slider's head/repeat/tail (+0.02) for the large-tick
/// hit results; misses scale with the beatmap HP drain rate.
pub fn increase_for(result: HitResult, is_slider_tick: bool, hp_drain: f64) -> f64 {
    match result {
        HitResult::SmallTickMiss | HitResult::LargeTickMiss => {
            difficulty_range(hp_drain, -0.02, -0.075, -0.14)
        }
        HitResult::Miss => difficulty_range(hp_drain, -0.03, -0.125, -0.2),
        // Classic slider mechanics: this result comes from the tail.
        HitResult::SmallTickHit => 0.02,
        HitResult::SliderTailHit | HitResult::LargeTickHit => {
            if is_slider_tick {
                0.015
            } else {
                0.02
            }
        }
        HitResult::Meh => 0.002,
        HitResult::Ok => 0.011,
        HitResult::Great => 0.03,
        HitResult::SmallBonus => 0.0085,
        HitResult::LargeBonus => 0.01,
        _ => 0.0,
    }
}

/// `HitResult.IsBonus` (SmallBonus / LargeBonus).
pub fn is_bonus(result: HitResult) -> bool {
    matches!(result, HitResult::SmallBonus | HitResult::LargeBonus)
}

/// `DrainingHealthProcessor.ComputeDrainRate`: binary-search-ish solver
/// walking the PERFECT-play increase list (`IsSimulating` collection,
/// max results, bonus results excluded) until the lowest health reached
/// sits within 0.01 of the target. Break periods skip their whole
/// inter-object gap (`while Breaks[currentBreak].EndTime <= currentTime:
/// lastTime = currentTime`).
pub fn compute_drain_rate(
    increases: &[(f64, f64)],
    drain_start: f64,
    breaks: &[(f64, f64)],
    target_minimum: f64,
) -> f64 {
    if increases.len() <= 1 {
        return 0.0;
    }

    const MINIMUM_HEALTH_ERROR: f64 = 0.01;

    // `Beatmap.Breaks` iterated by EndTime; sort defensively (the decoder
    // keeps file order).
    let mut breaks: Vec<(f64, f64)> = breaks.to_vec();
    breaks.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let mut adjustment: i64 = 1;
    let mut result = 1.0f64;

    while adjustment > 0 {
        let mut current_health = 1.0f64;
        let mut lowest_health = 1.0f64;
        let mut current_break = 0usize;

        for (i, (time, amount)) in increases.iter().enumerate() {
            let last_time = if i > 0 { increases[i - 1].0 } else { drain_start };
            let mut last_time = last_time;

            while current_break < breaks.len() && breaks[current_break].1 <= *time {
                // Two hit objects separated by a break: no drain for the
                // full duration between them.
                last_time = *time;
                current_break += 1;
            }

            current_health -= (time - last_time) * result;
            lowest_health = lowest_health.min(current_health);
            current_health = (current_health + amount).min(1.0);

            // Common scenario for when the drain rate is definitely too harsh.
            if lowest_health < 0.0 {
                break;
            }
        }

        // Stop within a reasonable offset of the target.
        if (lowest_health - target_minimum).abs() <= MINIMUM_HEALTH_ERROR {
            break;
        }

        adjustment = adjustment.saturating_mul(2);
        result += 1.0 / adjustment as f64 * (lowest_health - target_minimum).signum();
    }

    result
}

/// Drainable time across `[a, b]`: the drain window minus every no-drain
/// period (lazer drains per Update frame with the frame clamped into the
/// drain window, and skips frames inside a no-drain period entirely).
pub fn drain_span(info: &HealthInfo, a: f64, b: f64) -> f64 {
    if info.drain_rate <= 0.0 || b <= a {
        return 0.0;
    }
    let a = a.max(info.drain_start).min(info.drain_end);
    let b = b.max(info.drain_start).min(info.drain_end);
    if b <= a {
        return 0.0;
    }
    let mut span = b - a;
    for &(p0, p1) in &info.no_drain {
        let o0 = p0.max(a);
        let o1 = p1.min(b);
        if o1 > o0 {
            span -= o1 - o0;
        }
    }
    span.max(0.0)
}

/// Health at time `t`: the latest event's clamped health minus the drain
/// up to `t` (the same reconstruction `HealthProcessor.Update` produces
/// between judgements).
pub fn health_at(info: &HealthInfo, t: f64) -> f64 {
    let n = info.events.partition_point(|e| e.0 <= t);
    let (hp, last_t) = if n == 0 {
        (1.0, info.drain_start)
    } else {
        let e = info.events[n - 1];
        (e.1, e.0)
    };
    (hp - info.drain_rate * drain_span(info, last_t, t)).clamp(0.0, 1.0)
}
