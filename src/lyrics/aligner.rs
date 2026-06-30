use crate::lyrics::{LyricsData, SyncedLyricLine};

/// Aligns unsynced lyrics to audio timing using character-based timing estimation.
/// Falls back to line-count-based segmentation if no timing info is available.
pub fn align_lyrics(plain: &str, duration_ms: u64) -> Vec<SyncedLyricLine> {
    let lines: Vec<&str> = plain
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return Vec::new();
    }

    let total_chars: usize = lines.iter().map(|l| l.len()).sum();
    if total_chars == 0 {
        return evenly_spaced(&lines, duration_ms);
    }

    let ms_per_char = duration_ms as f64 / total_chars as f64;
    let mut current_time = 0.0_f64;
    let mut aligned = Vec::with_capacity(lines.len());

    for line in &lines {
        // Each line gets time proportional to its character count, plus a small gap
        let line_duration = (line.len() as f64 * ms_per_char).max(500.0);
        aligned.push(SyncedLyricLine {
            time: current_time / 1000.0,
            text: line.to_string(),
        });
        current_time += line_duration;
    }

    aligned
}

fn evenly_spaced(lines: &[&str], duration_ms: u64) -> Vec<SyncedLyricLine> {
    if lines.is_empty() {
        return Vec::new();
    }
    let interval = duration_ms as f64 / lines.len() as f64;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| SyncedLyricLine {
            time: (i as f64 * interval) / 1000.0,
            text: line.to_string(),
        })
        .collect()
}

/// Processes a LyricsData object — if it has plain lyrics but no synced lyrics,
/// generates synced lyrics using the track length if available.
pub fn process_lyrics_data(data: &mut LyricsData, duration_ms: Option<u64>) {
    if !data.synced_lyrics.is_empty() {
        return;
    }
    if let Some(ref plain) = data.plain_lyrics {
        let duration = duration_ms.unwrap_or(240_000); // default 4 min
        data.synced_lyrics = align_lyrics(plain, duration);
    }
}

/// Parse LRC timestamp from "MM:SS.ss" format
pub fn parse_lrc_timestamp(ts: &str) -> Option<f64> {
    let ts = ts.trim_matches(|c: char| c == '[' || c == ']');
    let (mins, rest) = ts.split_once(':')?;
    let m: f64 = mins.parse().ok()?;
    let secs: f64 = rest.parse().ok()?;
    Some(m * 60.0 + secs)
}

/// Convert synced lyrics back to LRC format string
pub fn to_lrc(lines: &[SyncedLyricLine]) -> String {
    lines
        .iter()
        .map(|l| {
            let mins = (l.time / 60.0) as u64;
            let secs = l.time % 60.0;
            format!("[{:02}:{:06.3}] {}", mins, secs, l.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
