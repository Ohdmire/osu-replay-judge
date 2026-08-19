//! Legacy mod bitflags and their judgment-affecting behaviour + lazer V2 score multiplier.

pub mod legacy_bits {
    pub const NOFAIL: u32 = 1;
    pub const EASY: u32 = 1 << 1;
    pub const TOUCH_DEVICE: u32 = 1 << 2;
    pub const HIDDEN: u32 = 1 << 3;
    pub const HARDROCK: u32 = 1 << 4;
    pub const SUDDENDEATH: u32 = 1 << 5;
    pub const DOUBLETIME: u32 = 1 << 6;
    pub const RELAX: u32 = 1 << 7;
    pub const HALFTIME: u32 = 1 << 8;
    pub const NIGHTCORE: u32 = 1 << 9;
    pub const FLASHLIGHT: u32 = 1 << 10;
    pub const AUTO: u32 = 1 << 11;
    pub const SPUN_OUT: u32 = 1 << 12;
    pub const AUTOPILOT: u32 = 1 << 13;
    pub const PERFECT: u32 = 1 << 14;
    pub const SCOREV2: u32 = 1 << 29;
}

#[derive(Clone, Debug)]
pub struct Mods {
    pub no_fail: bool,
    pub easy: bool,
    pub hidden: bool,
    pub hard_rock: bool,
    pub sudden_death: bool,
    pub perfect: bool,
    pub double_time: bool,
    pub half_time: bool,
    pub nightcore: bool,
    pub flashlight: bool,
    pub spun_out: bool,
    pub score_v2: bool,
    pub touch_device: bool,

    /// Gameplay clock rate (DT/NC 1.5, HT 0.75).
    pub rate: f64,

    /// Stable replays get `ModClassic` appended (classic note lock + classic
    /// slider behaviour with default settings).
    pub classic: bool,
}

impl Mods {
    /// `OsuRuleset.ConvertFromLegacyMods` + ModClassic append for stable scores.
    pub fn from_legacy(mods: u32, is_legacy_score: bool) -> Result<Mods, String> {
        if mods & legacy_bits::RELAX != 0 {
            return Err("Relax replays are not supported".into());
        }
        if mods & legacy_bits::AUTOPILOT != 0 {
            return Err("Autopilot replays are not supported".into());
        }
        if mods & legacy_bits::AUTO != 0 {
            return Err("Autoplay replays are not supported".into());
        }
        if mods & legacy_bits::SPUN_OUT != 0 {
            return Err("Spun Out replays are not supported".into());
        }

        // NC takes precedence over DT; PF over SD.
        let double_time = mods & legacy_bits::DOUBLETIME != 0 && mods & legacy_bits::NIGHTCORE == 0;
        let nightcore = mods & legacy_bits::NIGHTCORE != 0;
        let sudden_death =
            mods & legacy_bits::SUDDENDEATH != 0 && mods & legacy_bits::PERFECT == 0;

        let rate = if double_time || nightcore {
            1.5
        } else if mods & legacy_bits::HALFTIME != 0 {
            0.75
        } else {
            1.0
        };

        Ok(Mods {
            no_fail: mods & legacy_bits::NOFAIL != 0,
            easy: mods & legacy_bits::EASY != 0,
            hidden: mods & legacy_bits::HIDDEN != 0,
            hard_rock: mods & legacy_bits::HARDROCK != 0,
            sudden_death,
            perfect: mods & legacy_bits::PERFECT != 0,
            double_time,
            half_time: mods & legacy_bits::HALFTIME != 0,
            nightcore,
            flashlight: mods & legacy_bits::FLASHLIGHT != 0,
            spun_out: false,
            score_v2: mods & legacy_bits::SCOREV2 != 0,
            touch_device: mods & legacy_bits::TOUCH_DEVICE != 0,
            rate,
            classic: is_legacy_score,
        })
    }

    /// `OsuScoreMultiplierCalculatorV2` for the mods this tool supports.
    pub fn score_multiplier_v2(&self) -> f64 {
        let mut multiplier = 1.0;

        if self.easy {
            multiplier *= 0.8;
        }
        if self.no_fail {
            multiplier *= 0.5;
        }
        if self.hard_rock {
            multiplier *= 1.09;
        }
        if self.double_time || self.nightcore {
            multiplier *= double_time_multiplier(1.5);
        }
        if self.half_time {
            multiplier *= half_time_multiplier(0.75);
        }
        if self.hidden {
            multiplier *= 1.04;
        }
        if self.flashlight {
            multiplier *= 1.2;
        }
        if self.classic {
            // ClassicNoteLock defaults to true.
            multiplier *= 0.985;
        }

        multiplier
    }

    pub fn describe(&self) -> String {
        let mut names = Vec::new();
        if self.no_fail { names.push("NF"); }
        if self.easy { names.push("EZ"); }
        if self.hidden { names.push("HD"); }
        if self.hard_rock { names.push("HR"); }
        if self.sudden_death { names.push("SD"); }
        if self.perfect { names.push("PF"); }
        if self.double_time { names.push("DT"); }
        if self.nightcore { names.push("NC"); }
        if self.half_time { names.push("HT"); }
        if self.flashlight { names.push("FL"); }
        if self.score_v2 { names.push("SV2"); }
        if self.touch_device { names.push("TD"); }
        if self.classic { names.push("Classic"); }
        names.join(",")
    }
}

fn double_time_multiplier(speed_change: f64) -> f64 {
    // Floor to the nearest multiple of 0.1.
    let value = (speed_change * 10.0) as i32 as f64 / 10.0;
    let penalty = if value != 1.5 && value != 1.0 { 0.01 } else { 0.0 };
    (value - 1.0) * 0.46 + 1.0 - penalty
}

fn half_time_multiplier(speed_change: f64) -> f64 {
    (speed_change * 20.0) as i32 as f64 / 20.0 * 1.4 - 0.5
}
