//! osu!standard replay judge: a Rust reimplementation of osu!lazer
//! judgement (per-event exact score/combo/accuracy/unstable rate).
//!
//! File parsing (.osu / .osr / storyboard), slider path math and the
//! post-decode processing pipeline live in the shared `osu-parse` crate
//! and are re-exported here, so existing `osu_replay_judge::beatmap` /
//! `::replay` / `::process` / `::path` / `::vec2` / `::mods` paths keep
//! working. This crate adds the judgement layer on top:
//! [`engine`] (frame-stable simulation loop), [`score`] (ScoreProcessor),
//! [`health`] (HealthProcessor).

pub use osu_parse::{beatmap, mods, path, process, replay, vec2};

pub mod engine;
pub mod health;
pub mod score;
