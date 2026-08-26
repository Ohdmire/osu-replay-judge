//! ScoreProcessor port: standardised score, combo, accuracy, hit events and
//! unstable rate, plus the "classic" score display conversion.

/// Port of `HitResult` (the subset reachable in osu!std).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HitResult {
    None,
    Miss,
    Meh,
    Ok,
    Good,
    Great,
    Perfect,
    LargeTickMiss,
    LargeTickHit,
    SmallTickMiss,
    SmallTickHit,
    SliderTailHit,
    SmallBonus,
    LargeBonus,
    IgnoreMiss,
    IgnoreHit,
}

pub mod hit_result_ext {
    use super::HitResult;

    pub fn increases_combo(r: HitResult) -> bool {
        affects_combo(r) && is_hit(r)
    }

    pub fn breaks_combo(r: HitResult) -> bool {
        affects_combo(r) && !is_hit(r)
    }

    pub fn affects_combo(r: HitResult) -> bool {
        matches!(
            r,
            HitResult::Miss
                | HitResult::Meh
                | HitResult::Ok
                | HitResult::Good
                | HitResult::Great
                | HitResult::Perfect
                | HitResult::LargeTickHit
                | HitResult::LargeTickMiss
                | HitResult::SliderTailHit
        )
    }

    pub fn affects_accuracy(r: HitResult) -> bool {
        is_scorable(r) && !is_bonus(r)
    }

    pub fn is_basic(r: HitResult) -> bool {
        is_scorable(r) && !is_tick(r) && !is_bonus(r)
    }

    pub fn is_tick(r: HitResult) -> bool {
        matches!(
            r,
            HitResult::LargeTickHit
                | HitResult::LargeTickMiss
                | HitResult::SmallTickHit
                | HitResult::SmallTickMiss
                | HitResult::SliderTailHit
        )
    }

    pub fn is_bonus(r: HitResult) -> bool {
        matches!(r, HitResult::SmallBonus | HitResult::LargeBonus)
    }

    pub fn is_miss(r: HitResult) -> bool {
        matches!(
            r,
            HitResult::IgnoreMiss | HitResult::Miss | HitResult::SmallTickMiss | HitResult::LargeTickMiss
        )
    }

    pub fn is_hit(r: HitResult) -> bool {
        !matches!(
            r,
            HitResult::None
                | HitResult::IgnoreMiss
                | HitResult::Miss
                | HitResult::SmallTickMiss
                | HitResult::LargeTickMiss
        )
    }

    pub fn is_scorable(r: HitResult) -> bool {
        // Note: IgnoreHit and IgnoreMiss are excluded.
        matches!(
            r,
            HitResult::Miss
                | HitResult::Meh
                | HitResult::Ok
                | HitResult::Good
                | HitResult::Great
                | HitResult::Perfect
                | HitResult::SmallTickHit
                | HitResult::SmallTickMiss
                | HitResult::LargeTickHit
                | HitResult::LargeTickMiss
                | HitResult::SmallBonus
                | HitResult::LargeBonus
                | HitResult::SliderTailHit
        )
    }

    /// `ScoreProcessor.GetBaseScoreForResult`.
    pub fn base_score(r: HitResult) -> f64 {
        match r {
            HitResult::SmallTickHit => 10.0,
            HitResult::LargeTickHit => 30.0,
            HitResult::SliderTailHit => 150.0,
            HitResult::Meh => 50.0,
            HitResult::Ok => 100.0,
            HitResult::Good => 200.0,
            HitResult::Great | HitResult::Perfect => 300.0,
            HitResult::SmallBonus => 10.0,
            HitResult::LargeBonus => 50.0,
            _ => 0.0,
        }
    }

    pub fn name(r: HitResult) -> &'static str {
        match r {
            HitResult::None => "none",
            HitResult::Miss => "miss",
            HitResult::Meh => "meh",
            HitResult::Ok => "ok",
            HitResult::Good => "good",
            HitResult::Great => "great",
            HitResult::Perfect => "perfect",
            HitResult::LargeTickMiss => "ltickmiss",
            HitResult::LargeTickHit => "ltick",
            HitResult::SmallTickMiss => "stickmiss",
            HitResult::SmallTickHit => "stick",
            HitResult::SliderTailHit => "tail",
            HitResult::SmallBonus => "sbonus",
            HitResult::LargeBonus => "lbonus",
            HitResult::IgnoreMiss => "ignoremiss",
            HitResult::IgnoreHit => "ignorehit",
        }
    }
}

/// A judgement definition (`Judgement` equivalent).
#[derive(Clone, Copy, Debug)]
pub struct Judgement {
    pub max_result: HitResult,
}

impl Judgement {
    /// `Judgement.MinResult` default switch.
    pub fn min_result(&self) -> HitResult {
        match self.max_result {
            HitResult::SmallBonus | HitResult::LargeBonus | HitResult::IgnoreHit => HitResult::IgnoreMiss,
            HitResult::SmallTickHit => HitResult::SmallTickMiss,
            HitResult::LargeTickHit => HitResult::LargeTickMiss,
            HitResult::SliderTailHit => HitResult::IgnoreMiss,
            _ => HitResult::Miss,
        }
    }
}

/// C# `Math.Round` (banker's rounding, MidpointRounding.ToEven).
pub fn round_to_even(v: f64) -> f64 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 {
        // Exactly midpoint: pick the even neighbour.
        let lower = v.floor();
        let upper = v.ceil();
        if lower as i64 % 2 == 0 {
            lower
        } else {
            upper
        }
    } else {
        r
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HitEvent {
    pub time_offset: f64,
    pub gameplay_rate: f64,
    pub result: HitResult,
    /// Whether the source hit object has non-empty hit windows (UR filter).
    pub has_windows: bool,
    pub raw_time: f64,
}

pub struct ScoreProcessor {
    // Stored maxima (from autoplay simulation).
    maximum_base_score: f64,
    maximum_combo_portion: f64,
    maximum_accuracy_judgement_count: i64,
    pub maximum_total_score: i64,
    pub maximum_combo: i32,
    pub maximum_statistics: Vec<(HitResult, i32)>,

    // Current values.
    current_base_score: f64,
    current_maximum_base_score: f64,
    current_accuracy_judgement_count: i64,
    current_combo_portion: f64,
    current_bonus_portion: f64,
    pub combo: i32,
    pub highest_combo: i32,
    pub statistics: Vec<(HitResult, i32)>,
    pub hit_events: Vec<HitEvent>,

    pub score_multiplier: f64,
}

impl ScoreProcessor {
    const COMBO_EXPONENT: f64 = 0.5;

    pub fn new(score_multiplier: f64) -> ScoreProcessor {
        ScoreProcessor {
            maximum_base_score: 0.0,
            maximum_combo_portion: 0.0,
            maximum_accuracy_judgement_count: 0,
            maximum_total_score: 0,
            maximum_combo: 0,
            maximum_statistics: Vec::new(),
            current_base_score: 0.0,
            current_maximum_base_score: 0.0,
            current_accuracy_judgement_count: 0,
            current_combo_portion: 0.0,
            current_bonus_portion: 0.0,
            combo: 0,
            highest_combo: 0,
            statistics: Vec::new(),
            hit_events: Vec::new(),
            score_multiplier,
        }
    }

    fn bump(&mut self, result: HitResult, delta: i32) {
        for (r, c) in self.statistics.iter_mut() {
            if *r == result {
                *c += delta;
                return;
            }
        }
        if delta > 0 {
            self.statistics.push((result, delta));
        }
    }

    fn count(&self, result: HitResult) -> i32 {
        self.statistics
            .iter()
            .find(|(r, _)| *r == result)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    }

    /// `ScoreProcessor.ApplyResultInternal`.
    pub fn apply_result(&mut self, judgement: Judgement, result: HitResult, raw_time: f64, time_offset: f64, has_windows: bool, gameplay_rate: f64) {
        use hit_result_ext::*;

        if increases_combo(result) {
            self.combo += 1;
        } else if breaks_combo(result) {
            self.combo = 0;
        }
        self.highest_combo = self.highest_combo.max(self.combo);

        if affects_accuracy(judgement.max_result) {
            self.current_maximum_base_score += base_score(judgement.max_result);
            self.current_accuracy_judgement_count += 1;
        }
        if affects_accuracy(result) {
            self.current_base_score += base_score(result);
        }

        if is_bonus(result) {
            self.current_bonus_portion += base_score(result);
        } else if is_scorable(result) {
            self.current_combo_portion += base_score(judgement.max_result)
                * (self.combo as f64).powf(Self::COMBO_EXPONENT);
        }

        self.bump(result, 1);

        self.hit_events.push(HitEvent {
            time_offset,
            gameplay_rate,
            result,
            has_windows,
            raw_time,
        });
    }

    pub fn accuracy(&self) -> f64 {
        if self.current_maximum_base_score > 0.0 {
            self.current_base_score / self.current_maximum_base_score
        } else {
            1.0
        }
    }

    fn combo_progress(&self) -> f64 {
        if self.maximum_combo_portion > 0.0 {
            self.current_combo_portion / self.maximum_combo_portion
        } else {
            1.0
        }
    }

    fn accuracy_progress(&self) -> f64 {
        if self.maximum_accuracy_judgement_count > 0 {
            self.current_accuracy_judgement_count as f64 / self.maximum_accuracy_judgement_count as f64
        } else {
            1.0
        }
    }

    fn compute_total_score_without_mods(&self) -> f64 {
        let accuracy = self.accuracy();
        500000.0 * accuracy * self.combo_progress()
            + 500000.0 * accuracy.powi(5) * self.accuracy_progress()
            + self.current_bonus_portion
    }

    /// `TotalScoreWithoutMods`.
    pub fn total_score_without_mods(&self) -> i64 {
        round_to_even(self.compute_total_score_without_mods()) as i64
    }

    /// `TotalScore` = round(without_mods * multiplier).
    pub fn total_score(&self) -> i64 {
        round_to_even(self.total_score_without_mods() as f64 * self.score_multiplier) as i64
    }

    /// Store the maxima after an autoplay simulation run.
    pub fn store_maxima(&mut self) {
        self.maximum_base_score = self.current_base_score;
        self.maximum_combo_portion = self.current_combo_portion;
        self.maximum_accuracy_judgement_count = self.current_accuracy_judgement_count;
        self.maximum_statistics = self.statistics.clone();
        self.maximum_total_score = self.total_score();
        self.maximum_combo = self.highest_combo;
    }

    /// Reset current values (keeping maxima).
    pub fn reset(&mut self) {
        self.current_base_score = 0.0;
        self.current_maximum_base_score = 0.0;
        self.current_accuracy_judgement_count = 0;
        self.current_combo_portion = 0.0;
        self.current_bonus_portion = 0.0;
        self.combo = 0;
        self.highest_combo = 0;
        self.statistics.clear();
        self.hit_events.clear();
    }

    pub fn get_count(&self, result: HitResult) -> i32 {
        self.count(result)
    }

    /// Sum of basic judgements in maximum statistics (for classic display).
    fn maximum_basic_object_count(&self) -> f64 {
        use hit_result_ext::is_basic;
        self.maximum_statistics
            .iter()
            .filter(|(r, _)| is_basic(*r))
            .map(|(_, c)| *c as f64)
            .sum()
    }

    /// `ScoreInfoExtensions.GetDisplayScore` classic conversion for osu!.
    pub fn classic_display_score(&self) -> i64 {
        let object_count = self.maximum_basic_object_count();
        round_to_even((object_count * object_count * 32.57 + 100000.0) * self.total_score() as f64 / 1000000.0) as i64
    }

    /// Unstable rate via Welford's online algorithm (`HitEventExtensions`).
    pub fn unstable_rate(&self) -> Option<f64> {
        let mut event_count = 0u64;
        let mut mean = 0.0f64;
        let mut sum_of_squares = 0.0f64;

        for e in &self.hit_events {
            // AffectsUnstableRate: hit windows non-empty && IsHit.
            if !e.has_windows || !hit_result_ext::is_hit(e.result) {
                continue;
            }

            event_count += 1;
            // `HitEventExtensions.CalculateUnstableRate` divides by the
            // gameplay rate "to account for TimeOffset scaling with
            // gameplay rate" — lazer's TimeOffset lives on the
            // rate-adjusted clock. This engine's offsets are already map
            // time (the rate-adjusted value divided by the rate), so the
            // division is pre-applied; dividing again would normalise
            // twice (UR off by exactly the rate on DT/HT replays).
            let current_value = e.time_offset;
            let next_mean = mean + (current_value - mean) / event_count as f64;
            sum_of_squares += (current_value - mean) * (current_value - next_mean);
            mean = next_mean;
        }

        if event_count == 0 {
            None
        } else {
            Some(10.0 * (sum_of_squares / event_count as f64).sqrt())
        }
    }
}
