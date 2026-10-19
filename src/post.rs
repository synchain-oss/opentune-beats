//! Post-processing: refinement, tempo segments, octave reconciliation, bars and warnings.
//!
//! The model returns a beat grid and a set of downbeats. Everything the caller reads beyond the
//! raw times is built here, from our own MIT code: the constant-tempo segments and their fits,
//! the factor-of-two reconciliation between neighbouring segments, the bar numbering and the
//! confidence and warning codes of `docs/output.md`.

use std::cmp::Ordering;

use crate::onset::OnsetEnvelope;
use crate::stats;

/// One beat as the model produced it.
pub struct RawBeat {
    pub time: f64,
    pub confidence: f64,
}

/// The parameters a run actually used.
pub struct Settings {
    pub refine: bool,
    pub max_refine_ms: f64,
    pub min_segment_beats: usize,
    pub duration: f64,
}

/// One beat of the final grid.
#[derive(Clone)]
pub struct Beat {
    /// The model's time, before refinement.
    pub raw: f64,
    /// The final time.
    pub time: f64,
    pub confidence: f64,
    pub is_downbeat: bool,
    pub bar: Option<u32>,
    pub beat_in_bar: Option<u32>,
    pub segment: usize,
}
/// A time signature as (numerator, denominator).
pub type Signature = (u32, u32);
/// A time signature with the share of downbeats that support it.
pub type SignatureScore = (u32, u32, f64);

/// A constant-tempo span.
pub struct Segment {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub first_beat: usize,
    pub last_beat: usize,
    pub bpm: f64,
    pub period: f64,
    pub signature: Signature,
    pub candidates: Vec<SignatureScore>,
    pub confidence: f64,
    pub residual_median_ms: f64,
    pub residual_p95_ms: f64,
    pub octave_note: Option<&'static str>,
}

/// A boundary between two segments.
pub struct TempoChange {
    pub at_sec: f64,
    pub from_bpm: f64,
    pub to_bpm: f64,
    pub kind: &'static str,
}

/// What the millisecond refinement did.
pub struct Refinement {
    pub applied: bool,
    pub global_offset_ms: f64,
    pub median_adjust_ms: f64,
    pub p95_adjust_ms: f64,
}

/// One entry of the `warnings` array.
pub struct Warning {
    pub code: &'static str,
    pub detail: String,
    pub segment: Option<usize>,
}

/// Everything the JSON document needs, derived from the model's grid.
pub struct Analysis {
    pub beats: Vec<Beat>,
    pub segments: Vec<Segment>,
    pub tempo_changes: Vec<TempoChange>,
    pub refinement: Refinement,
    pub warnings: Vec<Warning>,
    pub dominant_bpm: f64,
}

struct OctaveFix {
    first_time: f64,
    last_time: f64,
    note: &'static str,
}

/// Build the analysis from the raw grid.
pub fn build(
    raw: Vec<RawBeat>,
    downbeats: &[f64],
    envelope: Option<&OnsetEnvelope>,
    settings: &Settings,
) -> Analysis {
    let mut warnings: Vec<Warning> = Vec::new();
    let (mut beats, refinement) = refine_beats(sanitize(raw), envelope, settings, &mut warnings);

    let spans = detect_segments(&beats, settings.min_segment_beats);
    let (adjusted, fixes) = resolve_octaves(&beats, &spans, settings.min_segment_beats);
    beats = adjusted;
    let spans = if fixes.is_empty() {
        spans
    } else {
        detect_segments(&beats, settings.min_segment_beats)
    };

    let downbeat_indices = match_downbeats(&beats, downbeats);
    let (signature, candidates) = time_signature(&downbeat_indices);
    let segments = fit_segments(
        &beats,
        &spans,
        settings.duration,
        signature,
        candidates,
        &fixes,
    );
    assign_segments(&mut beats, &segments);
    assign_bars(&mut beats, &downbeat_indices, signature.0 as usize);

    if downbeat_indices.len() < 2 {
        warnings.push(warning(
            "NO_DOWNBEATS",
            "no reliable downbeat was found; bar and beatInBar are null",
            None,
        ));
    }
    if is_irregular(&downbeat_indices) {
        warnings.push(warning(
            "IRREGULAR_METER",
            "the bars do not fit one time signature",
            None,
        ));
    }
    if beats.len() < 8 || settings.duration < 8.0 {
        warnings.push(warning(
            "SHORT_INPUT",
            "the input is short; the tempo is only a rough estimate",
            None,
        ));
    }
    let dominant_bpm = dominant_tempo(&segments);
    if dominant_bpm > 170.0 {
        warnings.push(warning(
            "POSSIBLE_HALF_TIME",
            "the reading may be half the perceived tempo",
            None,
        ));
    }
    if dominant_bpm < 70.0 {
        warnings.push(warning(
            "POSSIBLE_DOUBLE_TIME",
            "the reading may be double the perceived tempo",
            None,
        ));
    }
    for segment in &segments {
        if segment.confidence < 0.5 {
            warnings.push(warning(
                "LOW_CONFIDENCE_SEGMENT",
                "the tempo fit is uncertain for this segment",
                Some(segment.index),
            ));
        }
    }
    warnings.sort_by(|left, right| match (left.segment, right.segment) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => a.cmp(&b),
    });

    let tempo_changes = tempo_changes(&segments);
    Analysis {
        beats,
        segments,
        tempo_changes,
        refinement,
        warnings,
        dominant_bpm,
    }
}

fn warning(code: &'static str, detail: &str, segment: Option<usize>) -> Warning {
    Warning {
        code,
        detail: detail.to_string(),
        segment,
    }
}

fn sanitize(raw: Vec<RawBeat>) -> Vec<Beat> {
    let mut points: Vec<(f64, f64)> = Vec::with_capacity(raw.len());
    for item in raw {
        if !item.time.is_finite() || item.time < 0.0 {
            continue;
        }
        points.push((item.time, item.confidence.clamp(0.0, 1.0)));
    }
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut beats: Vec<Beat> = Vec::with_capacity(points.len());
    for (time, confidence) in points {
        if beats.last().is_some_and(|last| time - last.raw < 0.001) {
            continue;
        }
        beats.push(Beat {
            raw: time,
            time,
            confidence,
            is_downbeat: false,
            bar: None,
            beat_in_bar: None,
            segment: 0,
        });
    }
    beats
}

/// Refine every beat, or explain in a warning why it was not refined.
fn refine_beats(
    mut beats: Vec<Beat>,
    envelope: Option<&OnsetEnvelope>,
    settings: &Settings,
    warnings: &mut Vec<Warning>,
) -> (Vec<Beat>, Refinement) {
    let idle = Refinement {
        applied: false,
        global_offset_ms: 0.0,
        median_adjust_ms: 0.0,
        p95_adjust_ms: 0.0,
    };
    if !settings.refine {
        warnings.push(warning("REFINE_SKIPPED", "disabled by --no-refine", None));
        return (beats, idle);
    }
    let Some(envelope) = envelope else {
        warnings.push(warning(
            "REFINE_SKIPPED",
            "the decoded audio is unavailable",
            None,
        ));
        return (beats, idle);
    };
    if envelope.is_silent() || beats.is_empty() {
        warnings.push(warning(
            "REFINE_SKIPPED",
            "the input has no attack to refine against",
            None,
        ));
        return (beats, idle);
    }
    let limit = settings.max_refine_ms / 1000.0;
    let shifts: Vec<f64> = beats
        .iter()
        .map(|beat| envelope.snap(beat.raw, settings.max_refine_ms))
        .collect();
    let global = stats::median(&shifts).clamp(-limit, limit);
    let mut adjustments: Vec<f64> = Vec::with_capacity(beats.len());
    for (beat, shift) in beats.iter_mut().zip(shifts.iter()) {
        let adjust = (shift - global).clamp(-limit, limit);
        beat.time = (beat.raw + global + adjust).max(0.0);
        adjustments.push(adjust.abs() * 1000.0);
    }
    let refinement = Refinement {
        applied: true,
        global_offset_ms: global * 1000.0,
        median_adjust_ms: stats::median(&adjustments),
        p95_adjust_ms: stats::percentile(&adjustments, 0.95),
    };
    (beats, refinement)
}

/// Contiguous runs of beats whose inter-beat interval is stable, split at a step change.
fn detect_segments(beats: &[Beat], min_beats: usize) -> Vec<(usize, usize)> {
    let count = beats.len();
    if count == 0 {
        return Vec::new();
    }
    if count < 6 {
        return vec![(0, count - 1)];
    }
    let times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
    let mut intervals: Vec<f64> = Vec::with_capacity(times.len());
    for pair in times.windows(2) {
        intervals.push(pair[1] - pair[0]);
    }
    let window = 4usize;
    let mut cuts: Vec<usize> = Vec::new();
    let mut boundary = window;
    while boundary + window <= intervals.len() {
        let left = stats::median(&intervals[boundary - window..boundary]);
        let right = stats::median(&intervals[boundary..boundary + window]);
        let jump = if left > 0.0 {
            (right / left - 1.0).abs()
        } else {
            0.0
        };
        if jump > 0.04 {
            let mut best = boundary;
            let mut best_jump = jump;
            let low = boundary.saturating_sub(2).max(window);
            let high = (boundary + 2).min(intervals.len() - window);
            for candidate in low..=high {
                let before = stats::median(&intervals[candidate - window..candidate]);
                let after = stats::median(&intervals[candidate..candidate + window]);
                let value = if before > 0.0 {
                    (after / before - 1.0).abs()
                } else {
                    0.0
                };
                if value > best_jump {
                    best = candidate;
                    best_jump = value;
                }
            }
            cuts.push(best);
            boundary = best + min_beats.max(1);
        } else {
            boundary += 1;
        }
    }
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for cut in cuts {
        if cut > start {
            spans.push((start, cut - 1));
            start = cut;
        }
    }
    spans.push((start, count - 1));
    merge_short(spans, min_beats)
}

/// Merge a span shorter than `min_beats` into its neighbour.
fn merge_short(mut spans: Vec<(usize, usize)>, min_beats: usize) -> Vec<(usize, usize)> {
    let minimum = min_beats.max(2);
    loop {
        if spans.len() < 2 {
            break;
        }
        let mut short: Option<usize> = None;
        for (index, span) in spans.iter().enumerate() {
            if span.1 - span.0 + 1 < minimum {
                short = Some(index);
                break;
            }
        }
        let Some(index) = short else {
            break;
        };
        if index == 0 {
            spans[1].0 = spans[0].0;
        } else {
            spans[index - 1].1 = spans[index].1;
        }
        spans.remove(index);
    }
    spans
}

/// Reconcile a segment whose tempo is a factor of two off its neighbours.
fn resolve_octaves(
    beats: &[Beat],
    spans: &[(usize, usize)],
    min_beats: usize,
) -> (Vec<Beat>, Vec<OctaveFix>) {
    if spans.len() < 2 {
        return (beats.to_vec(), Vec::new());
    }
    let periods: Vec<Option<f64>> = spans
        .iter()
        .map(|(first, last)| {
            let times: Vec<f64> = beats[*first..=*last].iter().map(|beat| beat.time).collect();
            stats::fit(&times).map(|(_, period)| period)
        })
        .collect();
    // The reference is the period that covers most of the time, so a short stretch read an octave
    // away does not pull it, and each span's own reading is part of the vote.
    let mut weights: Vec<(f64, f64)> = Vec::new();
    for (span, period) in spans.iter().zip(periods.iter()) {
        if let Some(period) = period {
            weights.push((*period, (span.1 - span.0 + 1) as f64 * period));
        }
    }
    let reference = stats::weighted_median(&weights);
    let mut out: Vec<Beat> = Vec::with_capacity(beats.len());
    let mut fixes: Vec<OctaveFix> = Vec::new();
    let mut cursor = 0usize;
    for (index, &(first, last)) in spans.iter().enumerate() {
        for beat in &beats[cursor..first] {
            out.push(beat.clone());
        }
        let note = octave_note(periods[index], reference, last - first + 1, min_beats);
        match note {
            Some("halved") => {
                for beat in beats[first..=last].iter().step_by(2) {
                    out.push(beat.clone());
                }
                fixes.push(OctaveFix {
                    first_time: beats[first].time,
                    last_time: beats[last].time,
                    note: "halved",
                });
            }
            Some("doubled") => {
                let slice = &beats[first..=last];
                for (offset, beat) in slice.iter().enumerate() {
                    out.push(beat.clone());
                    if offset + 1 < slice.len() {
                        out.push(midpoint(beat, &slice[offset + 1]));
                    }
                }
                fixes.push(OctaveFix {
                    first_time: beats[first].time,
                    last_time: beats[last].time,
                    note: "doubled",
                });
            }
            _ => {
                for beat in &beats[first..=last] {
                    out.push(beat.clone());
                }
            }
        }
        cursor = last + 1;
    }
    for beat in &beats[cursor..] {
        out.push(beat.clone());
    }
    (out, fixes)
}

fn octave_note(
    period: Option<f64>,
    reference: f64,
    length: usize,
    min_beats: usize,
) -> Option<&'static str> {
    if length < min_beats.max(4) || reference <= 0.0 {
        return None;
    }
    let ratio = period? / reference;
    if (0.45..=0.55).contains(&ratio) {
        return Some("halved");
    }
    if (1.9..=2.1).contains(&ratio) {
        return Some("doubled");
    }
    None
}

fn midpoint(left: &Beat, right: &Beat) -> Beat {
    Beat {
        raw: (left.raw + right.raw) / 2.0,
        time: (left.time + right.time) / 2.0,
        confidence: (left.confidence + right.confidence) / 2.0,
        is_downbeat: false,
        bar: None,
        beat_in_bar: None,
        segment: 0,
    }
}

/// Which beat carries each downbeat; a downbeat further than 60 ms from every beat is dropped.
fn match_downbeats(beats: &[Beat], downbeats: &[f64]) -> Vec<usize> {
    let mut indices: Vec<usize> = Vec::new();
    for time in downbeats {
        if !time.is_finite() {
            continue;
        }
        let mut best: Option<usize> = None;
        let mut distance = f64::INFINITY;
        for (index, beat) in beats.iter().enumerate() {
            let value = (beat.time - time).abs();
            if value < distance {
                distance = value;
                best = Some(index);
            }
        }
        if let Some(index) = best {
            if distance <= 0.06 {
                indices.push(index);
            }
        }
    }
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// The most common gap between downbeats, and the scored alternatives.
fn time_signature(downbeats: &[usize]) -> (Signature, Vec<SignatureScore>) {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for pair in downbeats.windows(2) {
        let gap = pair[1] - pair[0];
        if gap == 0 {
            continue;
        }
        match counts.iter_mut().find(|entry| entry.0 == gap) {
            Some(entry) => entry.1 += 1,
            None => counts.push((gap, 1)),
        }
    }
    if counts.is_empty() {
        return ((4, 4), vec![(4, 4, 0.0)]);
    }
    let total: usize = counts.iter().map(|entry| entry.1).sum();
    let mut candidates: Vec<SignatureScore> = counts
        .iter()
        .map(|entry| (entry.0 as u32, 4, entry.1 as f64 / total as f64))
        .collect();
    candidates.sort_by(|left, right| right.2.total_cmp(&left.2));
    ((candidates[0].0, candidates[0].1), candidates)
}

fn is_irregular(downbeats: &[usize]) -> bool {
    let mut gaps: Vec<usize> = Vec::new();
    for pair in downbeats.windows(2) {
        gaps.push(pair[1] - pair[0]);
    }
    gaps.sort_unstable();
    gaps.dedup();
    gaps.len() > 1
}

fn boundary(beats: &[Beat], spans: &[(usize, usize)], index: usize) -> f64 {
    let left = spans[index - 1].1;
    let right = spans[index].0;
    (beats[left].time + beats[right].time) / 2.0
}

fn fit_segments(
    beats: &[Beat],
    spans: &[(usize, usize)],
    duration: f64,
    signature: Signature,
    candidates: Vec<SignatureScore>,
    fixes: &[OctaveFix],
) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::with_capacity(spans.len());
    for (index, &(first, last)) in spans.iter().enumerate() {
        let mut times: Vec<f64> = Vec::with_capacity(last - first + 1);
        for beat in &beats[first..=last] {
            times.push(beat.time);
        }
        let (offset, period) = stats::fit(&times).unwrap_or((times[0], 0.5));
        let residuals = stats::residuals_ms(&times, offset, period);
        let residual_median = stats::median(&residuals);
        let residual_p95 = stats::percentile(&residuals, 0.95);
        let confidence: Vec<f64> = beats[first..=last]
            .iter()
            .map(|beat| beat.confidence)
            .collect();
        let mean_confidence = stats::mean(&confidence);
        let fit_factor = (1.0 - residual_p95 / 60.0).clamp(0.0, 1.0);
        let start = if index == 0 {
            0.0
        } else {
            boundary(beats, spans, index)
        };
        let end = if index + 1 == spans.len() {
            duration.max(beats[last].time)
        } else {
            boundary(beats, spans, index + 1)
        };
        let mut octave = None;
        for fix in fixes {
            if fix.first_time <= beats[last].time && fix.last_time >= beats[first].time {
                octave = Some(fix.note);
            }
        }
        segments.push(Segment {
            index,
            start,
            end,
            first_beat: first,
            last_beat: last,
            bpm: if period > 0.0 { 60.0 / period } else { 0.0 },
            period,
            signature,
            candidates: candidates.clone(),
            confidence: (mean_confidence * fit_factor).clamp(0.0, 1.0),
            residual_median_ms: residual_median,
            residual_p95_ms: residual_p95,
            octave_note: octave,
        });
    }
    segments
}

fn assign_segments(beats: &mut [Beat], segments: &[Segment]) {
    for (index, segment) in segments.iter().enumerate() {
        for beat in &mut beats[segment.first_beat..=segment.last_beat] {
            beat.segment = index;
        }
    }
}

fn assign_bars(beats: &mut [Beat], downbeats: &[usize], meter: usize) {
    for beat in beats.iter_mut() {
        beat.is_downbeat = false;
        beat.bar = None;
        beat.beat_in_bar = None;
    }
    if downbeats.len() < 2 {
        return;
    }
    for (bar, &start) in downbeats.iter().enumerate() {
        let end = downbeats.get(bar + 1).copied().unwrap_or(beats.len());
        for index in start..end {
            if let Some(beat) = beats.get_mut(index) {
                beat.bar = Some(bar as u32 + 1);
                beat.beat_in_bar = Some((index - start) as u32 + 1);
                beat.is_downbeat = index == start;
            }
        }
    }
    let last_start = downbeats[downbeats.len() - 1];
    let meter = meter.max(2);
    for index in last_start..beats.len() {
        let offset = index - last_start;
        if let Some(beat) = beats.get_mut(index) {
            beat.bar = Some((downbeats.len() - 1 + offset / meter) as u32 + 1);
            beat.beat_in_bar = Some((offset % meter) as u32 + 1);
            beat.is_downbeat = offset.is_multiple_of(meter);
        }
    }
}

fn dominant_tempo(segments: &[Segment]) -> f64 {
    let mut weights: Vec<(f64, f64)> = Vec::with_capacity(segments.len());
    for segment in segments {
        let length = (segment.last_beat - segment.first_beat + 1) as f64;
        weights.push((segment.bpm, length));
    }
    stats::weighted_median(&weights)
}

fn tempo_changes(segments: &[Segment]) -> Vec<TempoChange> {
    let mut changes: Vec<TempoChange> = Vec::new();
    for pair in segments.windows(2) {
        let from = pair[0].bpm;
        let to = pair[1].bpm;
        let ratio = if from > 0.0 {
            (to / from - 1.0).abs()
        } else {
            0.0
        };
        changes.push(TempoChange {
            at_sec: pair[1].start,
            from_bpm: from,
            to_bpm: to,
            kind: if ratio >= 0.08 { "step" } else { "ramp" },
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(start: f64, period: f64, count: usize) -> Vec<RawBeat> {
        (0..count)
            .map(|index| RawBeat {
                time: start + index as f64 * period,
                confidence: 0.9,
            })
            .collect()
    }

    fn settings(duration: f64) -> Settings {
        Settings {
            refine: false,
            max_refine_ms: 30.0,
            min_segment_beats: 4,
            duration,
        }
    }

    #[test]
    fn a_single_tempo_becomes_one_segment() {
        let beats = grid(0.5, 0.5, 16);
        let analysis = build(beats, &[], None, &settings(8.5));
        assert_eq!(analysis.segments.len(), 1);
        let segment = &analysis.segments[0];
        assert!((segment.bpm - 120.0).abs() < 1e-6);
        assert_eq!(segment.first_beat, 0);
        assert_eq!(segment.last_beat, 15);
        assert!((analysis.dominant_bpm - 120.0).abs() < 1e-6);
    }

    #[test]
    fn a_step_change_splits_the_segments() {
        let mut beats = grid(0.5, 0.5, 16);
        beats.extend(grid(0.5 + 16.0 * 0.5, 0.4, 16));
        let analysis = build(beats, &[], None, &settings(14.0));
        assert_eq!(analysis.segments.len(), 2);
        assert!((analysis.segments[0].bpm - 120.0).abs() < 1.0);
        assert!((analysis.segments[1].bpm - 150.0).abs() < 1.0);
        assert_eq!(analysis.tempo_changes.len(), 1);
    }

    #[test]
    fn a_double_time_segment_is_halved() {
        let mut beats = grid(0.5, 0.5, 16);
        beats.extend(grid(8.5, 0.25, 32));
        beats.extend(grid(16.5, 0.5, 16));
        let analysis = build(beats, &[], None, &settings(24.5));
        let notes: Vec<Option<&str>> = analysis
            .segments
            .iter()
            .map(|segment| segment.octave_note)
            .collect();
        assert!(notes.contains(&Some("halved")));
        for segment in &analysis.segments {
            assert!(segment.bpm < 130.0);
        }
    }

    #[test]
    fn downbeats_number_the_bars() {
        let beats = grid(0.5, 0.5, 16);
        let downbeats = [0.5, 2.5, 4.5, 6.5, 8.5];
        let analysis = build(beats, &downbeats, None, &settings(8.5));
        assert!(analysis.beats[0].is_downbeat);
        assert_eq!(analysis.beats[0].bar, Some(1));
        assert_eq!(analysis.beats[0].beat_in_bar, Some(1));
        assert_eq!(analysis.beats[3].beat_in_bar, Some(4));
        assert_eq!(analysis.beats[4].bar, Some(2));
        assert_eq!(analysis.beats[4].beat_in_bar, Some(1));
        assert!(!analysis.beats[1].is_downbeat);
        assert!(analysis.warnings.iter().all(|w| w.code != "NO_DOWNBEATS"));
    }

    #[test]
    fn no_downbeats_leaves_bar_and_beat_in_bar_null() {
        let analysis = build(grid(0.5, 0.5, 16), &[], None, &settings(8.5));
        assert!(analysis.beats.iter().all(|beat| beat.bar.is_none()));
        assert!(analysis.beats.iter().all(|beat| beat.beat_in_bar.is_none()));
        assert!(analysis
            .warnings
            .iter()
            .any(|warning| warning.code == "NO_DOWNBEATS"));
    }

    #[test]
    fn a_silent_envelope_skips_refinement() {
        let envelope = crate::onset::compute(&[0.0f32; 48_000], 48_000);
        let analysis = build(grid(0.5, 0.5, 8), &[], Some(&envelope), &settings(4.5));
        assert!(!analysis.refinement.applied);
        assert!(analysis
            .warnings
            .iter()
            .any(|warning| warning.code == "REFINE_SKIPPED"));
    }
}
