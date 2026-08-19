use osu_replay_judge::{
    beatmap, engine::Engine, mods::Mods, process, replay, score::hit_result_ext, score::HitResult,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: osu_replay_judge <beatmap.osu> <replay.osr> [--mode classic|lazer] [--timeline <file>] [--json]");
        std::process::exit(2);
    }

    let beatmap_path = &args[1];
    let replay_path = &args[2];

    let mut force_mode: Option<String> = None;
    let mut timeline_path: Option<String> = None;
    let mut json = false;

    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                i += 1;
                force_mode = args.get(i).cloned();
            }
            "--timeline" => {
                i += 1;
                timeline_path = args.get(i).cloned();
            }
            "--json" => json = true,
            other => {
                eprintln!("unknown argument: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let beatmap_content = match std::fs::read_to_string(beatmap_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: cannot read beatmap: {}", e);
            std::process::exit(1);
        }
    };

    let map = match beatmap::decode(&beatmap_content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: cannot decode beatmap: {}", e);
            std::process::exit(1);
        }
    };

    let rep = match replay::decode_file(replay_path, map.version) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot decode replay: {}", e);
            std::process::exit(1);
        }
    };

    let is_legacy_score = (rep.header.version as i64) < 30_000_000;
    let classic = match force_mode.as_deref() {
        Some("classic") => true,
        Some("lazer") => false,
        _ => is_legacy_score,
    };

    let mods = match Mods::from_legacy(rep.header.mods, classic) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    };

    let difficulty = process::apply_difficulty_mods(map.difficulty, mods.hard_rock, mods.easy);
    let processed = process::process(&map, difficulty, classic);

    let mut engine = Engine::new(processed, &mods);
    engine.run(&rep.frames);

    let score = &engine.score;

    // Validation against the replay header (meaningful for classic mode /
    // stable replays).
    let n300 = score.get_count(HitResult::Great);
    let n100 = score.get_count(HitResult::Ok);
    let n50 = score.get_count(HitResult::Meh);
    let nmiss = score.get_count(HitResult::Miss);

    if json {
        println!("{{");
        println!("  \"player\": {:?}", rep.header.player_name);
        println!("  \"mods\": {:?}", mods.describe());
        println!("  \"mode\": {:?}", if classic { "classic" } else { "lazer" });
        println!("  \"total_score\": {}", score.total_score());
        println!("  \"classic_display_score\": {}", score.classic_display_score());
        println!("  \"max_combo\": {}", score.highest_combo);
        println!("  \"accuracy\": {}", score.accuracy());
        match score.unstable_rate() {
            Some(ur) => println!("  \"unstable_rate\": {}", ur),
            None => println!("  \"unstable_rate\": null"),
        }
        println!("  \"counts\": {{ \"300\": {}, \"100\": {}, \"50\": {}, \"miss\": {} }}", n300, n100, n50, nmiss);
        println!("  \"replay_header\": {{ \"score\": {}, \"max_combo\": {}, \"300\": {}, \"100\": {}, \"50\": {}, \"miss\": {} }}",
            rep.header.total_score, rep.header.max_combo, rep.header.count300, rep.header.count100, rep.header.count50, rep.header.misses);
        println!("  \"events\": {}", score.hit_events.len());
        println!("}}");
    } else {
        println!("=== osu! replay judgement ({} mode) ===", if classic { "classic" } else { "lazer" });
        println!("player: {}", rep.header.player_name);
        println!("mods:   {} (rate {}x, score multiplier {:.4})", mods.describe(), mods.rate, mods.score_multiplier_v2());
        println!();
        println!("total score (standardised): {}", score.total_score());
        println!("total score (classic view): {}", score.classic_display_score());
        println!("max combo: {}", score.highest_combo);
        println!("accuracy:  {:.6}%", score.accuracy() * 100.0);
        match score.unstable_rate() {
            Some(ur) => println!("unstable rate: {:.4}", ur),
            None => println!("unstable rate: n/a"),
        }
        println!("counts: 300={} 100={} 50={} miss={}", n300, n100, n50, nmiss);
        println!();
        println!("--- replay header (stable record) ---");
        println!(
            "score={} combo={} 300={} 100={} 50={} miss={}",
            rep.header.total_score, rep.header.max_combo, rep.header.count300, rep.header.count100, rep.header.count50, rep.header.misses
        );
    }

    if let Some(path) = timeline_path {
        let mut out = String::from("time\tobject\tlabel\tresult\tcombo\tscore\tclassic_score\taccuracy\tunstable_rate\toffset\n");
        for e in &engine.timeline {
            out.push_str(&format!(
                "{:.4}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{}\t{:.4}\n",
                e.time,
                e.object_index,
                e.label,
                hit_result_ext::name(e.result),
                e.combo,
                e.score,
                e.classic_score,
                e.accuracy,
                e.unstable_rate.map(|v| format!("{:.4}", v)).unwrap_or_else(|| "n/a".into()),
                e.time_offset,
            ));
        }
        if let Err(e) = std::fs::write(&path, out) {
            eprintln!("error: cannot write timeline: {}", e);
            std::process::exit(1);
        }
        if !json {
            eprintln!("timeline written to {}", path);
        }
    }
}
