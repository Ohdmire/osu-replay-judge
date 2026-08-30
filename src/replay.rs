//! .osr replay file parsing (port of `LegacyScoreDecoder` + `SerializationReader`).

use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub struct ReplayFrame {
    /// Absolute time in beatmap milliseconds.
    pub time: f64,
    pub position: (f32, f32),
    /// Pressed buttons (left bit0, right bit1; bits 2/3 map to the same buttons).
    pub left: bool,
    pub right: bool,
    /// Smoke bit (bit 4, `ReplayButtonState.Smoke`; default key C). Not a
    /// gameplay action, but the key overlay renders it.
    pub smoke: bool,
}

#[derive(Clone, Debug)]
pub struct ReplayHeader {
    pub game_mode: u8,
    pub version: i32,
    pub beatmap_md5: String,
    pub player_name: String,
    pub count300: u16,
    pub count100: u16,
    pub count50: u16,
    pub geki: u16,
    pub katu: u16,
    pub misses: u16,
    pub total_score: i32,
    pub max_combo: u16,
    pub mods: u32,
    /// Raw Windows FILETIME ticks (100 ns since 0001-01-01) for when the
    /// score was set, exactly as stored in the .osr header.
    pub timestamp: u64,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn u8(&mut self) -> Result<u8, String> {
        if self.pos >= self.data.len() {
            return Err("unexpected end of replay".into());
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }

    fn u32(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.data.len() {
            return Err("unexpected end of replay".into());
        }
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    fn u16(&mut self) -> Result<u16, String> {
        if self.pos + 2 > self.data.len() {
            return Err("unexpected end of replay".into());
        }
        let v = u16::from_le_bytes(self.data[self.pos..self.pos + 2].try_into().unwrap());
        self.pos += 2;
        Ok(v)
    }

    fn u64(&mut self) -> Result<u64, String> {
        if self.pos + 8 > self.data.len() {
            return Err("unexpected end of replay".into());
        }
        let v = u64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    fn bool(&mut self) -> Result<bool, String> {
        Ok(self.u8()? != 0)
    }

    /// .NET BinaryWriter string: 0x0b marker + 7-bit-encoded length + UTF-8.
    fn string(&mut self) -> Result<String, String> {
        let marker = self.u8()?;
        if marker == 0 {
            return Ok(String::new());
        }
        if marker != 0x0b {
            return Err(format!("invalid string marker 0x{:02x}", marker));
        }

        let mut length = 0usize;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            length |= ((b & 0x7f) as usize) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
        }

        if self.pos + length > self.data.len() {
            return Err("string length out of bounds".into());
        }
        let s = String::from_utf8_lossy(&self.data[self.pos..self.pos + length]).into_owned();
        self.pos += length;
        Ok(s)
    }

    /// int32 length prefix + bytes.
    fn byte_array(&mut self) -> Result<Vec<u8>, String> {
        let len = self.i32()?;
        if len <= 0 {
            return Ok(Vec::new());
        }
        if self.pos + len as usize > self.data.len() {
            return Err("byte array out of bounds".into());
        }
        let v = self.data[self.pos..self.pos + len as usize].to_vec();
        self.pos += len as usize;
        Ok(v)
    }
}

pub struct Replay {
    pub header: ReplayHeader,
    pub frames: Vec<ReplayFrame>,
    pub random_seed: Option<u32>,
}

pub fn decode(data: &[u8], beatmap_version: i32) -> Result<Replay, String> {
    let mut r = Reader::new(data);

    let game_mode = r.u8()?;
    let version = r.i32()?;
    let beatmap_md5 = r.string()?;
    let player_name = r.string()?;
    let _replay_md5 = r.string()?;

    let count300 = r.u16()?;
    let count100 = r.u16()?;
    let count50 = r.u16()?;
    let geki = r.u16()?;
    let katu = r.u16()?;
    let misses = r.u16()?;

    let total_score = r.i32()?;
    let max_combo = r.u16()?;
    let _perfect = r.bool()?;
    let mods = r.u32()?;
    let _hp_graph = r.string()?;
    let timestamp = r.u64()?;

    let compressed = r.byte_array()?;

    if version >= 20140721 {
        let _online_id = r.u64()?;
    } else if version >= 20121008 {
        let _online_id = r.u32()?;
    }

    // Lazer replays embed an extra compressed score-info block; not needed.
    let _ = r.byte_array();

    let header = ReplayHeader {
        game_mode,
        version,
        beatmap_md5,
        player_name,
        count300,
        count100,
        count50,
        geki,
        katu,
        misses,
        total_score,
        max_combo,
        mods,
        timestamp,
    };

    if game_mode != 0 {
        return Err(format!("only osu!standard replays are supported (mode {})", game_mode));
    }

    let frames_blob = lzma_decode(&compressed)?;
    let frames_str = String::from_utf8_lossy(&frames_blob);

    let (frames, random_seed) = parse_frames(&frames_str, beatmap_version)?;

    Ok(Replay { header, frames, random_seed })
}

fn lzma_decode(data: &[u8]) -> Result<Vec<u8>, String> {
    use lzma_rs::lzma_decompress;

    let mut input = std::io::Cursor::new(data.to_vec());
    let mut output: Vec<u8> = Vec::new();
    lzma_decompress(&mut input, &mut output).map_err(|e| format!("lzma error: {:?}", e))?;
    Ok(output)
}

fn parse_frames(blob: &str, beatmap_version: i32) -> Result<(Vec<ReplayFrame>, Option<u32>), String> {
    let beatmap_offset = if beatmap_version < 5 { 24.0f64 } else { 0.0 };

    let mut frames: Vec<ReplayFrame> = Vec::new();
    let mut last_time = beatmap_offset;
    let mut random_seed = None;

    for part in blob.split(',') {
        if part.is_empty() {
            continue;
        }
        let split: Vec<&str> = part.split('|').collect();
        if split.len() != 4 {
            continue;
        }

        // Random seed frame: "-12345|0|0|0".
        if split[0] == "-12345" {
            random_seed = split[3].trim().parse::<u32>().ok();
            continue;
        }

        let delta = parse_frame_time(split[0])?;
        last_time += delta;

        let x: f32 = split[1].trim().parse::<f32>().map_err(|_| "bad frame x")?;
        let y: f32 = split[2].trim().parse::<f32>().map_err(|_| "bad frame y")?;
        let buttons: u32 = split[3].trim().parse::<u32>().map_err(|_| "bad frame buttons")?;

        // ReplayButtonState: Left1=1, Right1=2, Left2=4, Right2=8, Smoke=16.
        let left = buttons & 0b0101 != 0;
        let right = buttons & 0b1010 != 0;
        let smoke = buttons & 0b10000 != 0;

        frames.push(ReplayFrame { time: last_time, position: (x, y), left, right, smoke });
    }

    // Stable replay quirks (LegacyScoreDecoder.readLegacyReplay).
    if frames.len() > 2 {
        if frames[1].time < frames[0].time {
            let mut f0 = frames[0].clone();
            f0.time = 0.0;
            frames[0] = f0;
        }

        if frames[0].time > frames[2].time {
            let t = frames[2].time;
            frames[0].time = t;
            frames[1].time = t;
        }
    }

    // Remove leading fake frames at (256, -500).
    while let Some(first) = frames.first() {
        if first.position == (256.0, -500.0) && frames.len() > 1 && frames[1].position != (256.0, -500.0) {
            frames.remove(0);
        } else {
            break;
        }
    }

    // Drop backwards-time frames.
    let mut cleaned: Vec<ReplayFrame> = Vec::with_capacity(frames.len());
    let mut last = f64::NEG_INFINITY;
    for f in frames {
        if f.time < last {
            continue;
        }
        last = f.time;
        cleaned.push(f);
    }

    Ok((cleaned, random_seed))
}

fn parse_frame_time(s: &str) -> Result<f64, String> {
    let t = s.trim().parse::<f64>().map_err(|_| "bad frame time")?;
    if t.fract() != 0.0 {
        // Old fractional lazer frames round to int.
        Ok(t.round())
    } else {
        Ok(t)
    }
}

/// Convenience: read a file and parse.
pub fn decode_file(path: &str, beatmap_version: i32) -> Result<Replay, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {}", path, e))?;
    decode(&data, beatmap_version)
}

/// Silence unused import warning for Read (kept for future streaming use).
#[allow(dead_code)]
fn _unused(_: impl Read) {}
