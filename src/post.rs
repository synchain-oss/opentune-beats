//! Post-processing: refinement, tempo segments, octave reconciliation, bars and warnings.
//!
//! The model returns a beat grid and a set of downbeats. Everything the caller reads beyond the
//! raw times is built here, from our own MIT code: the constant-tempo segments and their fits,
//! the factor-of-two reconciliation between neighbouring segments, the bar numbering and the
//! confidence and warning codes of `docs/output.md`.
//!
//! Two invariants of this module are worth stating up front:
//!
//! * A beat is a beat: the model reports no beat while a breakdown is silent, so the fit fills
//!   the *grid index* of the missing beats (`round(gap / period)`) instead of treating the gap
//!   as one very long beat. The reported `beats` array never gains an invented beat.
//! * The millisecond refinement never makes the grid less regular: a span adopts its refined
//!   times only when they are clearly better and the one-beat-interval spread does not grow past
//!   the envelope's own 1 ms resolution (see `refine_beats`), so it cannot turn a steady song
//!   into a jittery one.

use std::cmp::Ordering;

use crate::onset::OnsetEnvelope;
use crate::stats;

// ---------------------------------------------------------------------------------------------
// Tuning constants. Each one is here, with its reason, rather than at its use site.

/// The confidence below which a segment is reported as a low-confidence segment, both in
/// `warnings` and in `summary.lowConfidenceSegments`. One definition, so the two never drift.
pub const LOW_CONFIDENCE_THRESHOLD: f64 = 0.5;

/// True when a segment of this confidence is a low-confidence segment. A confidence exactly at the
/// threshold is not low (`docs/output.md`: "below the threshold").
pub fn is_low_confidence(confidence: f64) -> bool {
    confidence < LOW_CONFIDENCE_THRESHOLD
}

/// The local tempo window, in beats, used to decide where one constant-tempo span ends.
const SEGMENT_WINDOW: usize = 4;
/// The relative local-tempo change (over `SEGMENT_WINDOW` beats) that opens a new span. 4 % is
/// below the change a listener hears as a new tempo, so a drift within a section stays one span.
const SEGMENT_JUMP_RATIO: f64 = 0.04;

/// Two adjacent spans whose fitted tempos differ by less than this are candidates for merging.
/// 1.5 % is roughly one part in 67: two spans that close are the same tempo read twice, and the
/// caller's own tempo threshold (30 ms per beat) is coarser than that.
const MERGE_BPM_RATIO: f64 = 0.015;
/// A merged span is only kept when one straight line still fits it: the p95 residual of the
/// merged fit must stay below this. 60 ms is the scale the confidence term uses for a "poor" fit,
/// so a merge never hides a real tempo change behind a wide tolerance.
const MERGE_MAX_RESIDUAL_P95_MS: f64 = 60.0;

/// A gap of more than this many missing beats is a breakdown, not a dropped beat: the spans on
/// either side are detected and fitted separately, and are merged again when the tempo agrees.
/// Eight beats is two bars of 4/4, the shortest silence in which a tempo change is plausible.
const MAX_GAP_BEATS: u64 = 8;
/// Half-width of the window a missing-beat count is estimated from, in intervals.
const GAP_WINDOW: usize = 4;
/// Upper bound on an estimated missing-beat count, so a corrupt interval cannot overflow.
const MAX_STEP_COUNT: u64 = 4096;

/// A refinement must beat the current grid by at least this much, in milliseconds of either the
/// fit-residual p95, the inter-beat-interval spread or the beats' distance to their attacks,
/// before its span adopts it. The model's frame is 20 ms and the onset envelope is 1 ms, so a
/// gain under 2 ms is within the noise.
const REFINE_MIN_GAIN_MS: f64 = 2.0;
/// The onset envelope is sampled every millisecond, so two beats moved onto their attacks can
/// land up to that far from a machine-steady spacing. The refined interval spread may therefore
/// exceed the raw one by this slack; more than that means the snap is chasing unrelated attacks,
/// and the span keeps the model's times.
const REFINE_SPREAD_SLACK_MS: f64 = 2.0;

/// The confidence formula: three 0..1 terms, weighted. See `segment_confidence` for the meaning
/// of each term and why the weights are what they are.
const CONFIDENCE_MODEL_WEIGHT: f64 = 0.4;
const CONFIDENCE_FIT_WEIGHT: f64 = 0.4;
const CONFIDENCE_INLIER_WEIGHT: f64 = 0.2;
/// The model term never falls below this: a low activation is a weak signal, not a broken grid.
const CONFIDENCE_MODEL_FLOOR: f64 = 0.7;
/// The residual p95, in milliseconds, at which the fit term reaches zero.
const CONFIDENCE_RESIDUAL_SCALE_MS: f64 = 160.0;
/// A beat further than this from the fitted line counts as an outlier for the inlier term.
const CONFIDENCE_OUTLIER_MS: f64 = 80.0;
/// The outlier share at which the inlier term reaches zero.
const CONFIDENCE_OUTLIER_CAP: f64 = 0.5;

/// `IRREGULAR_METER` needs at least this many misaligned bar gaps, and at least this share of
/// them. One stray downbeat is not a meter; a section in another meter is.
const IRREGULAR_MIN_GAPS: usize = 2;
const IRREGULAR_MIN_SHARE: f64 = 0.25;

/// The relative tempo change across a short boundary that is called a step rather than a ramp.
const RAMP_STEP_RATIO: f64 = 0.08;
/// A local tempo drift smaller than this share of the mean interval is a ramp, not a step.
const RAMP_MIN_CHANGE: f64 = 0.02;
/// When one interval-to-interval jump covers this share of the total change, the change is a step.
const RAMP_MAX_JUMP: f64 = 0.5;

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
    /// True when the span this beat belongs to adopted the refined times.
    pub refined: bool,
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
    /// `"applied"` when the refinement was adopted for this span, `"skipped"` otherwise.
    pub refinement: &'static str,
    /// The time between this segment's last beat and the next segment's first beat when the model
    /// reported no beat in between; `None` when the two segments are contiguous.
    pub gap_after_sec: Option<f64>,
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
    /// True when at least one segment adopted the refined times.
    pub applied: bool,
    /// The beat-count-weighted median of the common offsets of the segments whose refinement was
    /// adopted, in ms; 0 when no segment was adopted. With one segment it is that segment's
    /// offset. With several segments of which only some were adopted, segments that kept the
    /// model's times do not count, so it is not a shift every beat received.
    pub global_offset_ms: f64,
    /// The median and p95 of the per-beat displacements that were applied beyond their own
    /// segment's common offset (not beyond `global_offset_ms`), in ms.
    pub median_adjust_ms: f64,
    pub p95_adjust_ms: f64,
    pub segments_applied: usize,
    pub segments_skipped: usize,
    /// The p95-p5 spread of the one-beat intervals before and after the refinement.
    pub ioi_spread_before_ms: f64,
    pub ioi_spread_after_ms: f64,
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

/// A constant-tempo fit over a span: `time = offset + grid_index * period`.
struct Fit {
    offset: f64,
    period: f64,
    /// The grid index of every beat of the span, starting at zero.
    indices: Vec<f64>,
}

/// Build the analysis from the raw grid.
///
/// Fails when fewer than two usable beats remain after the beats were cleaned, refined and
/// reconciled: there is no grid to describe then. `Err` carries the number of beats left.
pub fn build(
    raw: Vec<RawBeat>,
    downbeats: &[f64],
    envelope: Option<&OnsetEnvelope>,
    settings: &Settings,
) -> Result<Analysis, usize> {
    let mut warnings: Vec<Warning> = Vec::new();
    let beats = sanitize(raw);
    if beats.len() < 2 {
        return Err(beats.len());
    }

    // The refinement decides per span, so the spans of the raw grid are detected first.
    let raw_times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
    let steps = beat_steps(&raw_times);
    let spans = detect_segments(&beats, &steps, settings.min_segment_beats);
    let (refined, mut refinement) =
        refine_beats(beats, &spans, &steps, envelope, settings, &mut warnings);
    let mut beats = normalize(refined, settings.duration);
    if beats.len() < 2 {
        return Err(beats.len());
    }

    // The regularity guard: the per-span checks bound the interval spread inside a span, but a
    // span boundary can still move, so the whole grid is compared once more. Whenever the
    // refinement ran, the refined grid may not have widened the one-beat-interval spread by more
    // than the envelope's own millisecond resolution; past that the model's times win.
    let final_times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
    let before_times: Vec<f64> = beats
        .iter()
        .map(|beat| beat.raw.clamp(0.0, settings.duration.max(0.0)))
        .collect();
    let check_steps = beat_steps(&final_times);
    let before = ioi_spread_ms(&before_times, &check_steps);
    let after = ioi_spread_ms(&final_times, &check_steps);
    let mut reported_before = before;
    let mut reported_after = after;
    if refinement.applied && after > before + REFINE_SPREAD_SLACK_MS {
        fall_back_to_raw(&mut beats, &mut refinement);
        beats = normalize(beats, settings.duration);
        let times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
        let raw_times: Vec<f64> = beats
            .iter()
            .map(|beat| beat.raw.clamp(0.0, settings.duration.max(0.0)))
            .collect();
        let steps = beat_steps(&times);
        reported_before = ioi_spread_ms(&raw_times, &steps);
        reported_after = ioi_spread_ms(&times, &steps);
        warnings.push(warning(
            "REFINE_SKIPPED",
            "the onset snap would have made the grid less regular",
            None,
        ));
    }
    refinement.ioi_spread_before_ms = reported_before;
    refinement.ioi_spread_after_ms = reported_after;

    let times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
    let steps = beat_steps(&times);
    let spans = detect_segments(&beats, &steps, settings.min_segment_beats);
    let (adjusted, fixes) = resolve_octaves(&beats, &spans, &steps, settings.min_segment_beats);
    beats = adjusted;
    if beats.len() < 2 {
        return Err(beats.len());
    }
    let (spans, steps) = if fixes.is_empty() {
        (spans, steps)
    } else {
        let times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
        let steps = beat_steps(&times);
        let spans = detect_segments(&beats, &steps, settings.min_segment_beats);
        (spans, steps)
    };

    let matched = match_downbeats(&beats, downbeats);
    let (signature, candidates) = time_signature(&matched);
    let downbeat_indices = filter_downbeats(&matched, signature.0 as usize);
    let segments = fit_segments(
        &beats,
        &steps,
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
    if is_irregular(&downbeat_indices, signature.0 as usize) {
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
    // A fast reading may be double the perceived tempo, a slow one half of it.
    if dominant_bpm > 170.0 {
        warnings.push(warning(
            "POSSIBLE_DOUBLE_TIME",
            "the reading may be double the perceived tempo",
            None,
        ));
    }
    if dominant_bpm < 70.0 {
        warnings.push(warning(
            "POSSIBLE_HALF_TIME",
            "the reading may be half the perceived tempo",
            None,
        ));
    }
    for segment in &segments {
        if is_low_confidence(segment.confidence) {
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

    refinement.segments_applied = segments
        .iter()
        .filter(|segment| segment.refinement == "applied")
        .count();
    refinement.segments_skipped = segments.len() - refinement.segments_applied;
    refinement.applied = refinement.segments_applied > 0;

    let tempo_changes = tempo_changes(&beats, &steps, &segments);
    Ok(Analysis {
        beats,
        segments,
        tempo_changes,
        refinement,
        warnings,
        dominant_bpm,
    })
}

/// Undo the refinement as a whole: every beat goes back to the model's time, and the offsets the
/// rejected refinement reported are cleared so `globalOffsetMs` and the adjustment statistics
/// read 0, as the output contract says when no segment adopted refined times.
fn fall_back_to_raw(beats: &mut [Beat], refinement: &mut Refinement) {
    for beat in beats.iter_mut() {
        beat.time = beat.raw;
        beat.refined = false;
    }
    refinement.global_offset_ms = 0.0;
    refinement.median_adjust_ms = 0.0;
    refinement.p95_adjust_ms = 0.0;
}

/// Put the refined beats back in time order inside `[0, duration]` and drop a beat that lands
/// within 1 ms of the one before it, so `index` counts in time order and the grid never has a
/// zero or negative interval.
fn normalize(mut beats: Vec<Beat>, duration: f64) -> Vec<Beat> {
    for beat in &mut beats {
        beat.time = beat.time.clamp(0.0, duration.max(0.0));
    }
    beats.sort_by(|left, right| left.time.total_cmp(&right.time));
    let mut out: Vec<Beat> = Vec::with_capacity(beats.len());
    for beat in beats {
        if out.last().is_some_and(|last| beat.time - last.time < 0.001) {
            continue;
        }
        out.push(beat);
    }
    out
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
            refined: false,
            is_downbeat: false,
            bar: None,
            beat_in_bar: None,
            segment: 0,
        });
    }
    beats
}

// ---------------------------------------------------------------------------------------------
// Missing beats: the grid index of every beat

/// The number of beat periods each interval spans.
///
/// The model reports no beat while a breakdown is silent, so an interval there is a multiple of
/// the period, not one beat. The period to divide by comes from the intervals on the side the
/// interval itself agrees with: a breakdown returns to the tempo it left, while an abrupt tempo
/// change stays at the new one. Taking the median of both sides would read the first interval of
/// a sudden slow section as a missing-beat gap and bend the whole fit. Entry `i` is the step from
/// beat `i` to beat `i + 1`; the last entry is unused and stays 1.
fn beat_steps(times: &[f64]) -> Vec<u64> {
    let count = times.len();
    let mut steps = vec![1u64; count];
    if count < 2 {
        return steps;
    }
    let intervals: Vec<f64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    for (index, interval) in intervals.iter().enumerate() {
        if !interval.is_finite() || *interval <= 0.0 {
            continue;
        }
        let low = index.saturating_sub(GAP_WINDOW);
        let high = (index + GAP_WINDOW).min(intervals.len() - 1);
        let before = if index > low {
            stats::median(&intervals[low..index])
        } else {
            0.0
        };
        let after = if index < high {
            stats::median(&intervals[index + 1..=high])
        } else {
            0.0
        };
        let reference = match (before > 0.0, after > 0.0) {
            (true, true) => {
                if (interval - after).abs() < (interval - before).abs() {
                    after
                } else {
                    before
                }
            }
            (true, false) => before,
            (false, true) => after,
            (false, false) => continue,
        };
        let value = (interval / reference).round();
        if value.is_finite() && value >= 1.0 {
            steps[index] = (value as u64).min(MAX_STEP_COUNT);
        }
    }
    steps
}

/// The spread (p95 - p5) of the one-beat intervals, in milliseconds.
///
/// Only intervals that span a single beat take part: an interval across a gap would otherwise
/// dominate a number whose whole purpose is to describe how steady the beat is. `steps` is the
/// step from `times[i]` to `times[i + 1]`; a shorter slice is allowed and the missing tail is
/// treated as one beat.
fn ioi_spread_ms(times: &[f64], steps: &[u64]) -> f64 {
    let mut intervals: Vec<f64> = Vec::new();
    for index in 0..times.len().saturating_sub(1) {
        if steps.get(index).copied().unwrap_or(1) == 1 {
            intervals.push((times[index + 1] - times[index]) * 1000.0);
        }
    }
    if intervals.len() < 2 {
        return 0.0;
    }
    (stats::percentile(&intervals, 0.95) - stats::percentile(&intervals, 0.05)).max(0.0)
}

// ---------------------------------------------------------------------------------------------
// Refinement

/// Refine every span, or explain in a warning why it was not refined.
///
/// The model reads the audio in 20 ms frames, so its raw times carry up to half a frame of
/// quantization plus whatever the tracker did; the onset envelope finds the real attacks at 1 ms.
/// A span is refined in two steps, the same two the output documents:
///
/// 1. **A common offset.** The median of the per-beat snap shifts is the offset the attacks
///    suggest for the whole span; moving every beat by it leaves the interval spread exactly as
///    it was. This is what fixes a span the model placed one frame late, and it is reported as
///    `globalOffsetMs` (the median over the segments the refinement accepted).
/// 2. **A per-beat adjustment.** A beat moves further, onto its own attack, only when that brings
///    it *closer to the local tempo line through its neighbouring beats* (the mean of the beat
///    before and the beat after, or the span's line at an edge or across a gap). A strong attack
///    a little further away (a vocal, a syncopated stab) does not pull the beat off the grid,
///    which is what used to add the jitter.
///
/// A span is adopted only when it is clearly better somewhere by at least `REFINE_MIN_GAIN_MS`:
/// the fit-residual p95, the interval spread, or the median distance from the beats to their
/// attacks; and the interval spread may not grow by more than `REFINE_SPREAD_SLACK_MS` (the
/// envelope's own 1 ms resolution). A span that does not clear that bar keeps the model's times,
/// which is the whole point: on a steady song the old refinement made the grid worse.
fn refine_beats(
    mut beats: Vec<Beat>,
    spans: &[(usize, usize)],
    steps: &[u64],
    envelope: Option<&OnsetEnvelope>,
    settings: &Settings,
    warnings: &mut Vec<Warning>,
) -> (Vec<Beat>, Refinement) {
    let idle = Refinement {
        applied: false,
        global_offset_ms: 0.0,
        median_adjust_ms: 0.0,
        p95_adjust_ms: 0.0,
        segments_applied: 0,
        segments_skipped: 0,
        ioi_spread_before_ms: 0.0,
        ioi_spread_after_ms: 0.0,
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

    let mut adjustments: Vec<f64> = Vec::new();
    let mut applied_offsets: Vec<(f64, f64)> = Vec::new();
    let mut accepted = 0usize;
    let mut skipped = 0usize;
    for &(first, last) in spans {
        let Some(fit) = fit_span(&beats, steps, first, last) else {
            skipped += 1;
            continue;
        };
        let times = span_times(&beats, first, last);
        let span_shifts = &shifts[first..=last];
        // The common offset of this span, bounded by the refinement window like every other move.
        let offset = stats::median(span_shifts).clamp(-limit, limit);
        let mut candidate: Vec<f64> = Vec::with_capacity(times.len());
        for (position, index) in (first..=last).enumerate() {
            let raw = beats[index].raw;
            let base = (raw + offset).clamp(0.0, settings.duration.max(0.0));
            let target =
                (raw + shifts[index].clamp(-limit, limit)).clamp(0.0, settings.duration.max(0.0));
            // The local tempo line through the neighbouring beats: with a beat on each side one
            // period away it is their mean, otherwise (an edge, or a gap) the span's own line.
            // The line moves with the common offset, so the comparison below is about how far
            // this beat deviates from its neighbours, not about the common move itself.
            let local =
                if index > first && index < last && steps[index - 1] == 1 && steps[index] == 1 {
                    (beats[index - 1].raw + beats[index + 1].raw) / 2.0
                } else {
                    fit.offset + fit.indices[position] * fit.period
                };
            let line = local + offset;
            if (target - line).abs() < (base - line).abs() - 1e-12 {
                candidate.push(target);
            } else {
                candidate.push(base);
            }
        }
        let Some((candidate_offset, candidate_period)) =
            stats::fit_indexed(&fit.indices, &candidate)
        else {
            skipped += 1;
            continue;
        };
        let raw_p95 = stats::percentile(&residuals_at(&fit, &times), 0.95);
        let candidate_residuals =
            stats::residuals_at_ms(&fit.indices, &candidate, candidate_offset, candidate_period);
        let candidate_p95 = stats::percentile(&candidate_residuals, 0.95);
        let raw_spread = ioi_spread_ms(&times, &steps[first..last]);
        let candidate_spread = ioi_spread_ms(&candidate, &steps[first..last]);
        let moves: Vec<f64> = candidate
            .iter()
            .zip(times.iter())
            .map(|(moved, raw)| moved - raw)
            .collect();
        let raw_alignment = stats::median(
            &span_shifts
                .iter()
                .map(|shift| shift.abs())
                .collect::<Vec<f64>>(),
        ) * 1000.0;
        let candidate_alignment = stats::median(
            &span_shifts
                .iter()
                .zip(moves.iter())
                .map(|(shift, moved)| (shift - moved).abs())
                .collect::<Vec<f64>>(),
        ) * 1000.0;
        let residual_gain = raw_p95 - candidate_p95;
        let spread_gain = raw_spread - candidate_spread;
        let alignment_gain = raw_alignment - candidate_alignment;
        let clearly_better = residual_gain >= REFINE_MIN_GAIN_MS
            || spread_gain >= REFINE_MIN_GAIN_MS
            || alignment_gain >= REFINE_MIN_GAIN_MS;
        let never_worse = candidate_spread <= raw_spread + REFINE_SPREAD_SLACK_MS;
        if clearly_better && never_worse {
            for (position, index) in (first..=last).enumerate() {
                beats[index].time = candidate[position];
                beats[index].refined = true;
                // The adjustment is what this beat got beyond the segment's common offset.
                adjustments.push((moves[position] - offset).abs() * 1000.0);
            }
            applied_offsets.push((offset, times.len() as f64));
            accepted += 1;
        } else {
            skipped += 1;
        }
    }
    if accepted == 0 {
        warnings.push(warning(
            "REFINE_SKIPPED",
            "the onset snap did not improve any segment",
            None,
        ));
    }
    let refinement = Refinement {
        applied: accepted > 0,
        global_offset_ms: stats::weighted_median(&applied_offsets) * 1000.0,
        median_adjust_ms: stats::median(&adjustments),
        p95_adjust_ms: stats::percentile(&adjustments, 0.95),
        segments_applied: accepted,
        segments_skipped: skipped,
        ioi_spread_before_ms: 0.0,
        ioi_spread_after_ms: 0.0,
    };
    (beats, refinement)
}

// ---------------------------------------------------------------------------------------------
// Spans and their fits

fn span_times(beats: &[Beat], first: usize, last: usize) -> Vec<f64> {
    beats[first..=last].iter().map(|beat| beat.time).collect()
}

/// Least-squares fit of one span, using the grid index of every beat so a gap inside the span
/// does not bend the line.
fn fit_span(beats: &[Beat], steps: &[u64], first: usize, last: usize) -> Option<Fit> {
    if last <= first || last >= beats.len() {
        return None;
    }
    let mut indices = Vec::with_capacity(last - first + 1);
    indices.push(0.0);
    for step in steps.iter().take(last).skip(first) {
        let next = indices[indices.len() - 1] + (*step).max(1) as f64;
        indices.push(next);
    }
    let times = span_times(beats, first, last);
    let (offset, period) = stats::fit_indexed(&indices, &times)?;
    Some(Fit {
        offset,
        period,
        indices,
    })
}

fn residuals_at(fit: &Fit, times: &[f64]) -> Vec<f64> {
    stats::residuals_at_ms(&fit.indices, times, fit.offset, fit.period)
}

/// Contiguous runs of beats whose inter-beat interval is stable, split at a step change and at a
/// breakdown longer than `MAX_GAP_BEATS`.
fn detect_segments(beats: &[Beat], steps: &[u64], min_beats: usize) -> Vec<(usize, usize)> {
    let count = beats.len();
    if count == 0 {
        return Vec::new();
    }
    if count < 6 {
        return merge_short(vec![(0, count - 1)], min_beats);
    }

    // A long gap is where a breakdown can end in a different tempo: each side is detected on its
    // own, and `merge_similar` puts them back together when the tempo did not change.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (index, step) in steps.iter().enumerate().take(count - 1) {
        if *step > MAX_GAP_BEATS {
            runs.push((start, index));
            start = index + 1;
        }
    }
    runs.push((start, count - 1));

    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (first, last) in runs {
        // The interval each beat apart, with the missing beats divided out: a gap is not a tempo
        // change, so it must not look like one here.
        let periods: Vec<f64> = (first..last)
            .map(|index| {
                let step = steps[index].max(1) as f64;
                (beats[index + 1].time - beats[index].time) / step
            })
            .collect();
        let mut cursor = first;
        for cut in detect_cuts(&periods, min_beats) {
            let boundary = first + cut;
            if boundary > cursor {
                spans.push((cursor, boundary - 1));
                cursor = boundary;
            }
        }
        spans.push((cursor, last));
    }
    let spans = merge_short(spans, min_beats);
    merge_similar(beats, steps, spans)
}

/// The interval boundaries where the local tempo changes by more than `SEGMENT_JUMP_RATIO`.
fn detect_cuts(periods: &[f64], min_beats: usize) -> Vec<usize> {
    let mut cuts: Vec<usize> = Vec::new();
    let window = SEGMENT_WINDOW;
    if periods.len() < window * 2 {
        return cuts;
    }
    let mut boundary = window;
    while boundary + window <= periods.len() {
        let left = stats::median(&periods[boundary - window..boundary]);
        let right = stats::median(&periods[boundary..boundary + window]);
        let jump = relative_change(right, left);
        if jump > SEGMENT_JUMP_RATIO {
            // The cut is the split that leaves the least variance inside the two sides. A plain
            // "largest ratio" search reports the first interval that already sees the jump, which
            // is one beat early when the tempo drops; the change-point score puts the cut on the
            // beat where the new tempo actually starts.
            let low = boundary.saturating_sub(2).max(window);
            let high = (boundary + 2).min(periods.len() - window);
            let mut best = boundary;
            let mut best_score = split_error(
                &periods[boundary - window..boundary],
                &periods[boundary..boundary + window],
            );
            for candidate in low..=high {
                let before = &periods[candidate - window..candidate];
                let after = &periods[candidate..candidate + window];
                // A split whose two sides keep the same tempo is not a tempo change, however
                // small its variance (two steady windows before the change score 0 as well).
                if relative_change(stats::median(after), stats::median(before))
                    <= SEGMENT_JUMP_RATIO
                {
                    continue;
                }
                let score = split_error(before, after);
                let closer = candidate.abs_diff(boundary) < best.abs_diff(boundary);
                if score < best_score - 1e-12 || (score < best_score + 1e-12 && closer) {
                    best = candidate;
                    best_score = score;
                }
            }
            // The scan must move forward even when a short minimum segment puts the next window
            // back across the same change (min_beats 1 or 2), or it would find the same cut again
            // forever.
            if cuts.last().is_none_or(|&last| best > last) {
                cuts.push(best);
            }
            boundary = (best + min_beats.max(1)).max(boundary + 1);
        } else {
            boundary += 1;
        }
    }
    cuts
}

fn relative_change(value: f64, reference: f64) -> f64 {
    if reference > 0.0 {
        (value / reference - 1.0).abs()
    } else {
        0.0
    }
}

/// The summed squared deviation of two sides from their own means: zero when each side is one
/// constant tempo, larger when the split runs through the middle of a step.
fn split_error(left: &[f64], right: &[f64]) -> f64 {
    let mean_left = stats::mean(left);
    let mean_right = stats::mean(right);
    let mut error = 0.0;
    for value in left {
        error += (value - mean_left) * (value - mean_left);
    }
    for value in right {
        error += (value - mean_right) * (value - mean_right);
    }
    error
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

/// Merge neighbouring spans that read the same tempo, so a steady song ends up as one segment.
///
/// Two spans merge when their fitted tempos differ by less than `MERGE_BPM_RATIO` *and* one
/// straight line still fits the joined beats with a p95 residual under
/// `MERGE_MAX_RESIDUAL_P95_MS`. The second test is what keeps a tempo ramp or a real (if small)
/// tempo change apart: its two sides can be close in BPM, but they do not share one line.
fn merge_similar(
    beats: &[Beat],
    steps: &[u64],
    mut spans: Vec<(usize, usize)>,
) -> Vec<(usize, usize)> {
    loop {
        if spans.len() < 2 {
            break;
        }
        let mut merged = false;
        for index in 0..spans.len() - 1 {
            let Some(left) = fit_span(beats, steps, spans[index].0, spans[index].1) else {
                continue;
            };
            let Some(right) = fit_span(beats, steps, spans[index + 1].0, spans[index + 1].1) else {
                continue;
            };
            if relative_change(right.period, left.period) >= MERGE_BPM_RATIO {
                continue;
            }
            let first = spans[index].0;
            let last = spans[index + 1].1;
            let Some(combined) = fit_span(beats, steps, first, last) else {
                continue;
            };
            let times = span_times(beats, first, last);
            let residual_p95 = stats::percentile(&residuals_at(&combined, &times), 0.95);
            if residual_p95 <= MERGE_MAX_RESIDUAL_P95_MS {
                spans[index] = (first, last);
                spans.remove(index + 1);
                merged = true;
                break;
            }
        }
        if !merged {
            break;
        }
    }
    spans
}

/// Reconcile a segment whose tempo is a factor of two off its neighbours.
fn resolve_octaves(
    beats: &[Beat],
    spans: &[(usize, usize)],
    steps: &[u64],
    min_beats: usize,
) -> (Vec<Beat>, Vec<OctaveFix>) {
    if spans.len() < 2 {
        return (beats.to_vec(), Vec::new());
    }
    let periods: Vec<Option<f64>> = spans
        .iter()
        .map(|&(first, last)| fit_span(beats, steps, first, last).map(|fit| fit.period))
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
                // Doubling must not stop one beat short: when the next segment begins right after
                // this one, the interval to its first beat is one more of the doubled periods and
                // needs its midpoint. A real gap there is far longer and is left alone.
                if last + 1 < beats.len() {
                    let next = &beats[last + 1];
                    let period = periods[index].unwrap_or(0.0);
                    let gap = next.time - beats[last].time;
                    if period > 0.0 && gap > 0.0 && gap <= 1.6 * period {
                        out.push(midpoint(&beats[last], next));
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
        refined: left.refined && right.refined,
        is_downbeat: false,
        bar: None,
        beat_in_bar: None,
        segment: 0,
    }
}

// ---------------------------------------------------------------------------------------------
// Downbeats, bars and the time signature

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
///
/// The model marks a downbeat wherever its downbeat activation passes the threshold, so a bar
/// can carry an extra one. The dominant gap is still the meter; the alternatives keep the record
/// of what else was seen, and `filter_downbeats` removes the extra downbeats before the bars are
/// numbered.
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
    candidates.sort_by(|left, right| right.2.total_cmp(&left.2).then(right.0.cmp(&left.0)));
    // On a tie the longer bar wins: a 4/4 reading explains a stray downbeat on beat three as an
    // extra, while a 2/4 reading would have no room for it.
    let mut best = counts[0];
    for entry in &counts {
        if entry.1 > best.1 || (entry.1 == best.1 && entry.0 > best.0) {
            best = *entry;
        }
    }
    ((best.0 as u32, 4), candidates)
}

/// Drop downbeats that split one bar of the dominant meter in two. A downbeat that is early
/// relative to the last kept one, and whose successor is equally early, is one extra mark inside
/// a bar rather than the start of a new one; a real meter change makes both gaps large or
/// non-additive and is left for `is_irregular` to report.
fn filter_downbeats(downbeats: &[usize], meter: usize) -> Vec<usize> {
    let meter = meter.max(1);
    if downbeats.len() < 3 {
        return downbeats.to_vec();
    }
    let mut kept: Vec<usize> = Vec::with_capacity(downbeats.len());
    kept.push(downbeats[0]);
    for index in 1..downbeats.len() - 1 {
        let before = downbeats[index] - *kept.last().unwrap_or(&downbeats[0]);
        let after = downbeats[index + 1] - downbeats[index];
        if before < meter && after < meter && before + after == meter {
            continue;
        }
        kept.push(downbeats[index]);
    }
    kept.push(downbeats[downbeats.len() - 1]);
    if kept.len() >= 2 {
        kept
    } else {
        downbeats.to_vec()
    }
}

fn is_irregular(downbeats: &[usize], meter: usize) -> bool {
    if meter == 0 || downbeats.len() < 3 {
        return false;
    }
    let gaps: Vec<usize> = downbeats.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let misaligned = gaps
        .iter()
        .filter(|gap| **gap == 0 || **gap % meter != 0)
        .count();
    misaligned >= IRREGULAR_MIN_GAPS && misaligned as f64 >= IRREGULAR_MIN_SHARE * gaps.len() as f64
}

fn boundary(beats: &[Beat], spans: &[(usize, usize)], index: usize) -> f64 {
    let left = spans[index - 1].1;
    let right = spans[index].0;
    (beats[left].time + beats[right].time) / 2.0
}

fn fit_segments(
    beats: &[Beat],
    steps: &[u64],
    spans: &[(usize, usize)],
    duration: f64,
    signature: Signature,
    candidates: Vec<SignatureScore>,
    fixes: &[OctaveFix],
) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::with_capacity(spans.len());
    for (index, &(first, last)) in spans.iter().enumerate() {
        let times = span_times(beats, first, last);
        // A span has at least two beats at least 1 ms apart, so the fit holds; if it ever does
        // not, the span reports no tempo and zero confidence instead of an invented one.
        let fitted = fit_span(beats, steps, first, last);
        let (period, residuals) = match &fitted {
            Some(fit) => (fit.period, residuals_at(fit, &times)),
            None => (0.0, Vec::new()),
        };
        let residual_median = stats::median(&residuals);
        let residual_p95 = stats::percentile(&residuals, 0.95);
        let confidence: Vec<f64> = beats[first..=last]
            .iter()
            .map(|beat| beat.confidence)
            .collect();
        let mean_confidence = stats::mean(&confidence);
        let refined = beats[first..=last]
            .iter()
            .filter(|beat| beat.refined)
            .count();
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
        let gap_after_sec = if last + 1 < beats.len() && steps.get(last).copied().unwrap_or(1) > 1 {
            Some(beats[last + 1].time - beats[last].time)
        } else {
            None
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
            confidence: if fitted.is_some() {
                segment_confidence(mean_confidence, &residuals)
            } else {
                0.0
            },
            residual_median_ms: residual_median,
            residual_p95_ms: residual_p95,
            octave_note: octave,
            refinement: if refined > 0 { "applied" } else { "skipped" },
            gap_after_sec,
        });
    }
    segments
}

/// The 0..1 confidence of one segment, as the weighted mean of three readings:
///
/// * `model`: the mean of the model's per-beat probabilities, lifted from the bottom by
///   `CONFIDENCE_MODEL_FLOOR`. The model is trained across many genres and its frame
///   probabilities are not calibrated, so a mediocre activation must not, on its own, condemn a
///   segment that fits one line perfectly.
/// * `fit`: how far the beats stray from that line, with the residual p95 scaled by
///   `CONFIDENCE_RESIDUAL_SCALE_MS`; a tight fit scores near one, a wandering one near zero.
/// * `inliers`: the share of beats inside `CONFIDENCE_OUTLIER_MS` of the line, so a handful of
///   stray beats (or the two beats the model placed around a breakdown) barely move the number,
///   while a span where most beats are off the line still falls to zero.
///
/// The weights are model 0.4, fit 0.4, inliers 0.2: the two grid terms together outweigh the
/// model's own probability, because the grid terms are what the caller acts on.
fn segment_confidence(mean_confidence: f64, residuals: &[f64]) -> f64 {
    let model =
        CONFIDENCE_MODEL_FLOOR + (1.0 - CONFIDENCE_MODEL_FLOOR) * mean_confidence.clamp(0.0, 1.0);
    let fit =
        (1.0 - stats::percentile(residuals, 0.95) / CONFIDENCE_RESIDUAL_SCALE_MS).clamp(0.0, 1.0);
    let outliers = residuals
        .iter()
        .filter(|value| **value > CONFIDENCE_OUTLIER_MS)
        .count() as f64;
    let share = if residuals.is_empty() {
        0.0
    } else {
        outliers / residuals.len() as f64
    };
    let inliers = (1.0 - share / CONFIDENCE_OUTLIER_CAP).clamp(0.0, 1.0);
    (CONFIDENCE_MODEL_WEIGHT * model
        + CONFIDENCE_FIT_WEIGHT * fit
        + CONFIDENCE_INLIER_WEIGHT * inliers)
        .clamp(0.0, 1.0)
}

fn assign_segments(beats: &mut [Beat], segments: &[Segment]) {
    for (index, segment) in segments.iter().enumerate() {
        for beat in &mut beats[segment.first_beat..=segment.last_beat] {
            beat.segment = index;
        }
    }
}

/// Number the bars from the downbeats. A bar starts at a detected downbeat, and also every
/// `meter` beats when the model missed one, so a missing downbeat does not stretch one bar over
/// two.
fn assign_bars(beats: &mut [Beat], downbeats: &[usize], meter: usize) {
    for beat in beats.iter_mut() {
        beat.is_downbeat = false;
        beat.bar = None;
        beat.beat_in_bar = None;
    }
    if downbeats.len() < 2 {
        return;
    }
    let meter = meter.max(2);
    let mut cursor = 0usize;
    let mut bar = 1u32;
    let mut bar_start = downbeats[0];
    for (index, beat) in beats.iter_mut().enumerate().skip(downbeats[0]) {
        if cursor + 1 < downbeats.len() && index == downbeats[cursor + 1] {
            cursor += 1;
            bar += 1;
            bar_start = index;
        } else if index > bar_start && (index - bar_start).is_multiple_of(meter) {
            bar += 1;
            bar_start = index;
        }
        let offset = index - bar_start;
        beat.bar = Some(bar);
        beat.beat_in_bar = Some(offset as u32 + 1);
        beat.is_downbeat = offset == 0;
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

/// How the tempo moves across each segment boundary.
///
/// `"step"` is an abrupt change: the beats on one side sit on one tempo and the beats on the
/// other on another, so one interval-to-interval jump carries most of the difference. `"ramp"`
/// is a gradual one: the intervals drift across the boundary, no single jump dominates, and a
/// drift too small to matter (under `RAMP_MIN_CHANGE` of the mean interval) counts as gradual.
fn tempo_changes(beats: &[Beat], steps: &[u64], segments: &[Segment]) -> Vec<TempoChange> {
    let times: Vec<f64> = beats.iter().map(|beat| beat.time).collect();
    let mut changes: Vec<TempoChange> = Vec::new();
    for pair in segments.windows(2) {
        changes.push(TempoChange {
            at_sec: pair[1].start,
            from_bpm: pair[0].bpm,
            to_bpm: pair[1].bpm,
            kind: change_kind(&times, steps, &pair[0], &pair[1]),
        });
    }
    changes
}

fn change_kind(times: &[f64], steps: &[u64], left: &Segment, right: &Segment) -> &'static str {
    let mut intervals: Vec<f64> = Vec::new();
    for index in left.first_beat..left.last_beat {
        if steps[index] == 1 {
            intervals.push(times[index + 1] - times[index]);
        }
    }
    for index in right.first_beat..right.last_beat {
        if steps[index] == 1 {
            intervals.push(times[index + 1] - times[index]);
        }
    }
    let ratio = relative_change(right.period, left.period);
    if intervals.len() < 4 {
        return if ratio >= RAMP_STEP_RATIO {
            "step"
        } else {
            "ramp"
        };
    }
    // A median-of-three removes the single-beat jitter that would otherwise look like a jump.
    let smoothed: Vec<f64> = (0..intervals.len())
        .map(|index| {
            let low = index.saturating_sub(1);
            let high = (index + 1).min(intervals.len() - 1);
            stats::median(&intervals[low..=high])
        })
        .collect();
    let mean = stats::mean(&smoothed);
    if mean <= 0.0 {
        return "step";
    }
    let total = smoothed[smoothed.len() - 1] - smoothed[0];
    if total.abs() < RAMP_MIN_CHANGE * mean {
        return "ramp";
    }
    let mut max_jump = 0.0f64;
    for pair in smoothed.windows(2) {
        max_jump = max_jump.max((pair[1] - pair[0]).abs());
    }
    if max_jump >= RAMP_MAX_JUMP * total.abs() {
        "step"
    } else {
        "ramp"
    }
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

    fn jittered(start: f64, period: f64, offsets: &[f64], confidence: f64) -> Vec<RawBeat> {
        offsets
            .iter()
            .enumerate()
            .map(|(index, offset)| RawBeat {
                time: start + index as f64 * period + offset,
                confidence,
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

    fn has_warning(analysis: &Analysis, code: &str) -> bool {
        analysis.warnings.iter().any(|warning| warning.code == code)
    }

    fn spread_ms(times: &[f64]) -> f64 {
        let steps = beat_steps(times);
        ioi_spread_ms(times, &steps)
    }

    #[test]
    fn the_low_confidence_boundary_is_strict() {
        assert!(is_low_confidence(LOW_CONFIDENCE_THRESHOLD - 1e-9));
        assert!(!is_low_confidence(LOW_CONFIDENCE_THRESHOLD));
        assert!(!is_low_confidence(LOW_CONFIDENCE_THRESHOLD + 1e-9));
        assert!(is_low_confidence(0.0));
    }

    #[test]
    fn two_long_gaps_stay_one_segment() {
        // 100 BPM with two 7.2 s breakdowns (twelve missing beats each), the shape of the song
        // the card reports: the tempo must survive both and come back as one segment.
        let mut beats = Vec::new();
        let mut time = 0.5;
        for index in 0..64 {
            beats.push(RawBeat {
                time,
                confidence: 0.9,
            });
            time += 0.6;
            if index == 20 || index == 44 {
                time += 0.6 * 12.0;
            }
        }
        let analysis = build(beats, &[], None, &settings(time + 0.5)).expect("builds");
        assert_eq!(analysis.segments.len(), 1);
        assert!(
            (analysis.segments[0].bpm - 100.0).abs() < 0.5,
            "bpm {}",
            analysis.segments[0].bpm
        );
        assert_eq!(analysis.beats.len(), 64, "no beat is invented in a gap");
    }

    #[test]
    fn four_tempo_segments_are_all_found() {
        let tempos = [92.0, 110.0, 128.0, 100.0];
        let mut beats: Vec<RawBeat> = Vec::new();
        let mut time = 0.5;
        for (segment, bpm) in tempos.iter().enumerate() {
            let period = 60.0 / bpm;
            for _ in 0..48 {
                if !(segment == 0 && beats.is_empty()) {
                    time += period;
                }
                beats.push(RawBeat {
                    time,
                    confidence: 0.9,
                });
            }
        }
        let analysis = build(beats, &[], None, &settings(time + 0.5)).expect("builds");
        assert_eq!(analysis.segments.len(), tempos.len());
        for (segment, bpm) in analysis.segments.iter().zip(tempos) {
            assert!(
                (segment.bpm - bpm).abs() < 1.0,
                "segment bpm {} vs {bpm}",
                segment.bpm
            );
        }
    }

    #[test]
    fn an_abrupt_tempo_drop_is_not_a_gap() {
        // 120 BPM for eight beats, then 60 BPM for eight beats: every interval is one beat, so
        // no step may be read as a missing beat.
        let mut times: Vec<f64> = (0..8).map(|index| 0.5 + index as f64 * 0.5).collect();
        times.extend((0..8).map(|index| 4.0 + index as f64 * 1.0));
        let steps = beat_steps(&times);
        assert!(
            steps[..times.len() - 1].iter().all(|step| *step == 1),
            "{steps:?}"
        );
    }

    #[test]
    fn a_gap_of_several_beats_is_counted() {
        // 100 BPM with two beats missing after the third one.
        let times = vec![0.5, 1.1, 1.7, 3.5, 4.1, 4.7];
        let steps = beat_steps(&times);
        assert_eq!(steps[2], 3);
        assert_eq!(steps[3], 1);
    }

    #[test]
    fn a_single_tempo_becomes_one_segment() {
        let beats = grid(0.5, 0.5, 16);
        let analysis = build(beats, &[], None, &settings(8.5)).expect("builds");
        assert_eq!(analysis.segments.len(), 1);
        let segment = &analysis.segments[0];
        assert!((segment.bpm - 120.0).abs() < 1e-6);
        assert_eq!(segment.first_beat, 0);
        assert_eq!(segment.last_beat, 15);
        assert!((analysis.dominant_bpm - 120.0).abs() < 1e-6);
    }

    #[test]
    fn jitter_does_not_split_a_steady_tempo() {
        // A 100 BPM grid with the kind of +/-20 ms jitter a real song shows.
        let offsets = [
            0.012, -0.015, 0.008, -0.010, 0.018, -0.006, 0.004, -0.019, 0.011, -0.013, 0.016,
            -0.008, 0.002, -0.017, 0.009, -0.011, 0.014, -0.004, 0.006, -0.016, 0.013, -0.009,
            0.001, -0.018, 0.010, -0.012, 0.015, -0.007, 0.003, -0.014, 0.007, -0.005,
        ];
        let beats = jittered(0.5, 0.6, &offsets, 0.6);
        let analysis = build(beats, &[], None, &settings(20.0)).expect("builds");
        assert_eq!(
            analysis.segments.len(),
            1,
            "a steady tempo stays one segment"
        );
        assert!((analysis.segments[0].bpm - 100.0).abs() < 0.5);
        assert!(analysis.segments[0].confidence >= 0.8);
        assert!(!has_warning(&analysis, "IRREGULAR_METER"));
    }

    #[test]
    fn a_realistic_steady_grid_gets_a_high_confidence() {
        // The reported shape of a stable song: IOI p5/p50/p95 = 580/600/620 ms, model
        // confidence around 0.5, and two beats the model placed noticeably wrong.
        let mut beats = grid(0.5, 0.6, 48);
        for beat in beats.iter_mut() {
            beat.confidence = 0.5;
        }
        beats[7].time += 0.035;
        beats[30].time -= 0.045;
        let analysis = build(beats, &[], None, &settings(30.0)).expect("builds");
        assert_eq!(analysis.segments.len(), 1);
        assert!(
            analysis.segments[0].confidence >= 0.8,
            "confidence {}",
            analysis.segments[0].confidence
        );
        assert!(!has_warning(&analysis, "LOW_CONFIDENCE_SEGMENT"));
    }

    #[test]
    fn a_step_change_splits_the_segments() {
        let mut beats = grid(0.5, 0.5, 16);
        beats.extend(grid(0.5 + 16.0 * 0.5, 0.4, 16));
        let analysis = build(beats, &[], None, &settings(14.0)).expect("builds");
        assert_eq!(analysis.segments.len(), 2);
        assert!((analysis.segments[0].bpm - 120.0).abs() < 1.0);
        assert!((analysis.segments[1].bpm - 150.0).abs() < 1.0);
        assert_eq!(analysis.tempo_changes.len(), 1);
        assert_eq!(analysis.tempo_changes[0].kind, "step");
    }

    #[test]
    fn a_gradual_ramp_is_reported_as_a_ramp() {
        // The period shrinks by 1.2 % per beat: fast enough for the detector to cut, but each
        // interval is only a hair shorter than the one before, so no single jump dominates.
        let mut time = 0.5;
        let mut beats = Vec::new();
        let mut period = 0.85;
        for _ in 0..32 {
            beats.push(RawBeat {
                time,
                confidence: 0.9,
            });
            time += period;
            period -= 0.01;
        }
        let analysis = build(beats, &[], None, &settings(time + 0.5)).expect("builds");
        assert!(analysis.segments.len() >= 2, "the ramp is cut into spans");
        for change in &analysis.tempo_changes {
            assert_eq!(change.kind, "ramp", "change at {}", change.at_sec);
        }
        for pair in analysis.segments.windows(2) {
            assert!(
                pair[1].bpm >= pair[0].bpm,
                "tempo rises: {} then {}",
                pair[0].bpm,
                pair[1].bpm
            );
        }
    }

    #[test]
    fn a_double_time_segment_is_halved() {
        let mut beats = grid(0.5, 0.5, 16);
        beats.extend(grid(8.5, 0.25, 32));
        beats.extend(grid(16.5, 0.5, 16));
        let analysis = build(beats, &[], None, &settings(24.5)).expect("builds");
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
    fn a_doubled_segment_gains_the_midpoint_before_the_next_segment() {
        // The middle span is read at half tempo, so it is doubled: its own beats and the
        // midpoints between them, including the one that reaches the next segment.
        let mut beats = grid(0.5, 0.5, 16);
        beats.extend(grid(8.5, 1.0, 8));
        beats.extend(grid(16.5, 0.5, 16));
        let analysis = build(beats, &[], None, &settings(24.5)).expect("builds");
        assert!(
            analysis
                .segments
                .iter()
                .any(|segment| segment.octave_note == Some("doubled")),
            "the half-time span is doubled"
        );
        // 15.5 is the half-time span's last reported beat and 16.5 the next segment's first; the
        // grid must carry the beat between them, or the next segment starts one period late.
        assert!(
            analysis
                .beats
                .iter()
                .any(|beat| (beat.time - 16.0).abs() < 0.06),
            "the doubled span fills the interval before the next segment"
        );
    }

    #[test]
    fn downbeats_number_the_bars() {
        let beats = grid(0.5, 0.5, 16);
        let downbeats = [0.5, 2.5, 4.5, 6.5, 8.5];
        let analysis = build(beats, &downbeats, None, &settings(8.5)).expect("builds");
        assert!(analysis.beats[0].is_downbeat);
        assert_eq!(analysis.beats[0].bar, Some(1));
        assert_eq!(analysis.beats[0].beat_in_bar, Some(1));
        assert_eq!(analysis.beats[3].beat_in_bar, Some(4));
        assert_eq!(analysis.beats[4].bar, Some(2));
        assert_eq!(analysis.beats[4].beat_in_bar, Some(1));
        assert!(!analysis.beats[1].is_downbeat);
        assert!(!has_warning(&analysis, "NO_DOWNBEATS"));
    }

    #[test]
    fn no_downbeats_leaves_bar_and_beat_in_bar_null() {
        let analysis = build(grid(0.5, 0.5, 16), &[], None, &settings(8.5)).expect("builds");
        assert!(analysis.beats.iter().all(|beat| beat.bar.is_none()));
        assert!(analysis.beats.iter().all(|beat| beat.beat_in_bar.is_none()));
        assert!(has_warning(&analysis, "NO_DOWNBEATS"));
    }

    #[test]
    fn an_isolated_extra_downbeat_does_not_make_the_meter_irregular() {
        let beats = grid(0.5, 0.5, 32);
        // 4/4 downbeats every four beats, with two extra marks inside bars.
        let downbeats = [
            0.5, 2.5, 4.5, 6.5, 8.5, 10.5, 12.5, 14.5, 16.5, 18.5, 20.5, 22.5, 24.5, 26.5, 28.5,
            30.5,
        ];
        let analysis = build(beats, &downbeats, None, &settings(16.5)).expect("builds");
        assert_eq!(analysis.segments[0].signature, (4, 4));
        assert!(!has_warning(&analysis, "IRREGULAR_METER"));
        // The extra mark on beat 2 of the first bar is not a bar start.
        assert!(!analysis.beats[1].is_downbeat);
        assert!(analysis.beats[0].is_downbeat);
        assert!(analysis.beats[4].is_downbeat);
    }

    #[test]
    fn one_stray_downbeat_is_not_a_meter_change() {
        let beats = grid(0.5, 0.5, 32);
        let mut downbeats: Vec<f64> = (0..6).map(|bar| 0.5 + bar as f64 * 2.0).collect();
        downbeats.push(11.5);
        let analysis = build(beats, &downbeats, None, &settings(16.5)).expect("builds");
        assert!(!has_warning(&analysis, "IRREGULAR_METER"));
    }

    #[test]
    fn a_meter_change_is_irregular() {
        let beats = grid(0.5, 0.5, 32);
        // Four 4/4 bars, then three 3/4 bars: six of eight gaps disagree with the 4/4 reading.
        let mut downbeats: Vec<f64> = (0..5).map(|bar| 0.5 + bar as f64 * 2.0).collect();
        for bar in 0..4 {
            downbeats.push(8.5 + bar as f64 * 1.5);
        }
        let analysis = build(beats, &downbeats, None, &settings(16.5)).expect("builds");
        assert!(has_warning(&analysis, "IRREGULAR_METER"));
    }

    #[test]
    fn short_gaps_keep_the_beat_indices() {
        // Two gaps of three missing beats at 100 BPM: the tempo must not read as 40 BPM.
        let mut beats = Vec::new();
        let mut time = 0.5;
        for index in 0..48 {
            beats.push(RawBeat {
                time,
                confidence: 0.9,
            });
            let step = if index == 15 || index == 31 { 4 } else { 1 };
            time += 0.6 * step as f64;
        }
        let duration = time;
        let analysis = build(beats, &[], None, &settings(duration)).expect("builds");
        assert_eq!(analysis.segments.len(), 1);
        assert!(
            (analysis.segments[0].bpm - 100.0).abs() < 1.0,
            "bpm {}",
            analysis.segments[0].bpm
        );
        assert_eq!(analysis.beats.len(), 48, "no beat is invented in a gap");
        assert!(analysis.segments[0].gap_after_sec.is_none());
    }

    #[test]
    fn a_long_gap_with_a_tempo_change_splits_the_segments() {
        let mut beats = grid(0.5, 0.6, 16);
        // A twelve-beat breakdown, then the new tempo.
        let start = 0.5 + 15.0 * 0.6;
        beats.extend(grid(start + 12.0 * 0.6, 0.5, 16));
        let duration = start + 12.0 * 0.6 + 15.0 * 0.5 + 0.5;
        let analysis = build(beats, &[], None, &settings(duration)).expect("builds");
        assert_eq!(analysis.segments.len(), 2);
        assert!((analysis.segments[0].bpm - 100.0).abs() < 1.0);
        assert!((analysis.segments[1].bpm - 120.0).abs() < 1.0);
        let gap = analysis.segments[0]
            .gap_after_sec
            .expect("the gap is reported");
        assert!((gap - 7.2).abs() < 0.05, "gap {gap}");
        assert!(analysis.segments[1].gap_after_sec.is_none());
    }

    #[test]
    fn a_long_gap_at_the_same_tempo_merges_back_into_one_segment() {
        let mut beats = grid(0.5, 0.6, 16);
        let start = 0.5 + 15.0 * 0.6;
        beats.extend(grid(start + 15.0 * 0.6, 0.6, 16));
        let duration = start + 15.0 * 0.6 + 15.0 * 0.6 + 0.5;
        let analysis = build(beats, &[], None, &settings(duration)).expect("builds");
        assert_eq!(
            analysis.segments.len(),
            1,
            "a steady tempo stays one segment"
        );
        assert!((analysis.segments[0].bpm - 100.0).abs() < 1.0);
    }

    #[test]
    fn a_silent_envelope_skips_refinement() {
        let envelope = crate::onset::compute(&[0.0f32; 48_000], 48_000);
        let analysis =
            build(grid(0.5, 0.5, 8), &[], Some(&envelope), &settings(4.5)).expect("builds");
        assert!(!analysis.refinement.applied);
        assert_eq!(analysis.refinement.segments_applied, 0);
        assert!(has_warning(&analysis, "REFINE_SKIPPED"));
    }

    #[test]
    fn a_fast_reading_may_be_double_time() {
        let analysis = build(grid(0.5, 0.3, 24), &[], None, &settings(7.5)).expect("builds");
        assert!(analysis.dominant_bpm > 170.0);
        assert!(has_warning(&analysis, "POSSIBLE_DOUBLE_TIME"));
        assert!(!has_warning(&analysis, "POSSIBLE_HALF_TIME"));
    }

    #[test]
    fn a_slow_reading_may_be_half_time() {
        let analysis = build(grid(0.5, 1.0, 12), &[], None, &settings(12.5)).expect("builds");
        assert!(analysis.dominant_bpm < 70.0);
        assert!(has_warning(&analysis, "POSSIBLE_HALF_TIME"));
        assert!(!has_warning(&analysis, "POSSIBLE_DOUBLE_TIME"));
    }

    #[test]
    fn fewer_than_two_usable_beats_is_an_error() {
        let none = build(Vec::new(), &[], None, &settings(5.0));
        assert_eq!(none.err(), Some(0));
        // Negative and non-finite times are dropped by the cleaning.
        let dropped = vec![
            RawBeat {
                time: -1.0,
                confidence: 0.9,
            },
            RawBeat {
                time: f64::NAN,
                confidence: 0.9,
            },
            RawBeat {
                time: 1.0,
                confidence: 0.9,
            },
        ];
        assert_eq!(build(dropped, &[], None, &settings(5.0)).err(), Some(1));
        // Two beats within 1 ms are one beat.
        let twins = vec![
            RawBeat {
                time: 1.0,
                confidence: 0.9,
            },
            RawBeat {
                time: 1.0005,
                confidence: 0.9,
            },
        ];
        assert_eq!(build(twins, &[], None, &settings(5.0)).err(), Some(1));
    }

    #[test]
    fn two_beats_are_enough_for_a_segment() {
        let analysis = build(grid(0.5, 0.5, 2), &[], None, &settings(2.0)).expect("builds");
        assert_eq!(analysis.segments.len(), 1);
        assert!((analysis.segments[0].bpm - 120.0).abs() < 1e-6);
    }

    #[test]
    fn beats_stay_inside_the_input_and_in_order() {
        // The last two beats lie past the end of a 4.8 s input.
        let analysis = build(grid(0.5, 0.5, 12), &[], None, &settings(4.8)).expect("builds");
        let last = analysis.beats.last().expect("beats");
        assert!(last.time <= 4.8 + 1e-9);
        for pair in analysis.beats.windows(2) {
            assert!(pair[1].time - pair[0].time >= 0.001);
        }
        let segment = analysis.segments.last().expect("segments");
        assert!(segment.end <= 4.8 + 1e-9);
    }

    #[test]
    fn a_jittered_grid_is_refined_onto_its_attacks() {
        // The model's raw times carry +/-15 ms of jitter; the attacks sit on the true grid.
        let rate = 48_000_u32;
        let attacks = [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];
        let mut samples = vec![0.0f32; rate as usize * 5];
        for attack in attacks {
            let start = (attack * f64::from(rate)) as usize;
            for offset in 0..(rate as usize / 200) {
                let decay = (-(offset as f32) / (rate as f32 * 0.0015)).exp();
                samples[start + offset] += 0.8 * decay;
            }
        }
        let envelope = crate::onset::compute(&samples, rate);
        let offsets = [0.012, -0.014, 0.010, -0.008, 0.015, -0.011, 0.006, -0.013];
        let raw = jittered(0.5, 0.5, &offsets, 0.9);
        let mut on = settings(5.5);
        on.refine = true;
        let analysis = build(raw, &[], Some(&envelope), &on).expect("builds");
        assert!(analysis.refinement.applied);
        assert!(analysis.segments.iter().all(|s| s.refinement == "applied"));
        for (beat, attack) in analysis.beats.iter().zip(attacks) {
            assert!(
                (beat.time - attack).abs() < 0.012,
                "{} vs {attack}",
                beat.time
            );
            assert!((beat.time - beat.raw).abs() <= 0.030 + 1e-9);
        }
        assert!(
            analysis.refinement.ioi_spread_after_ms <= analysis.refinement.ioi_spread_before_ms
        );
    }

    #[test]
    fn a_common_offset_is_applied_without_adding_jitter() {
        // The model sits on a perfect line, but every attack is 25 ms later (one frame plus a
        // little). The refinement may follow that common offset, but it must not add jitter: the
        // beat-to-beat spacing has to stay as it was.
        let mut strength = vec![0.0f32; 6_000];
        for index in 0..10 {
            let beat = (0.5 + index as f64 * 0.5) * 1000.0;
            strength[(beat as usize) + 25] = 5.0;
        }
        let envelope = crate::onset::OnsetEnvelope::from_strength(0.001, strength);
        let raw = grid(0.5, 0.5, 10);
        let before: Vec<f64> = raw.iter().map(|beat| beat.time).collect();
        let mut on = settings(5.5);
        on.refine = true;
        let analysis = build(raw, &[], Some(&envelope), &on).expect("builds");
        assert!(analysis.refinement.applied);
        assert!(
            (analysis.refinement.global_offset_ms - 25.0).abs() < 1.0,
            "global offset {}",
            analysis.refinement.global_offset_ms
        );
        assert!(analysis.refinement.median_adjust_ms < 0.01);
        let after: Vec<f64> = analysis.beats.iter().map(|beat| beat.time).collect();
        assert!(
            spread_ms(&after) <= spread_ms(&before) + REFINE_SPREAD_SLACK_MS,
            "the spacing does not become less regular"
        );
        for (beat, time) in analysis.beats.iter().zip(before) {
            assert!((beat.time - time - 0.025).abs() < 0.002);
        }
    }

    #[test]
    fn a_short_minimum_segment_still_ends_the_cut_scan() {
        // 100 bpm then 120 bpm: with a minimum of 1 or 2 beats the window after a cut still sees
        // the old tempo, which used to report the same cut again without end.
        let mut periods = vec![0.6; 16];
        periods.extend(vec![0.5; 16]);
        for min_beats in 1..=3 {
            let cuts = detect_cuts(&periods, min_beats);
            assert_eq!(cuts, vec![16], "min_beats {min_beats}");
        }
    }

    #[test]
    fn falling_back_to_raw_clears_the_reported_offsets() {
        let mut beats: Vec<Beat> = (0..4)
            .map(|index| Beat {
                raw: index as f64 * 0.6,
                time: index as f64 * 0.6 + 0.012,
                confidence: 0.9,
                refined: true,
                is_downbeat: false,
                bar: None,
                beat_in_bar: None,
                segment: 0,
            })
            .collect();
        let mut refinement = Refinement {
            applied: true,
            global_offset_ms: 12.0,
            median_adjust_ms: 3.0,
            p95_adjust_ms: 7.0,
            segments_applied: 1,
            segments_skipped: 0,
            ioi_spread_before_ms: 0.0,
            ioi_spread_after_ms: 0.0,
        };
        fall_back_to_raw(&mut beats, &mut refinement);
        assert!(beats
            .iter()
            .all(|beat| !beat.refined && beat.time == beat.raw));
        assert_eq!(refinement.global_offset_ms, 0.0);
        assert_eq!(refinement.median_adjust_ms, 0.0);
        assert_eq!(refinement.p95_adjust_ms, 0.0);
    }

    #[test]
    fn refinement_follows_a_late_span_without_touching_a_steady_one() {
        // The first span is exactly on its attacks; the second is a whole frame late. Only the
        // second may move, and by its own common offset.
        let mut strength = vec![0.0f32; 20_000];
        for index in 0..8 {
            let beat = (0.5 + index as f64 * 0.5) * 1000.0;
            strength[beat as usize] = 5.0;
        }
        for index in 0..8 {
            let beat = (4.6 + index as f64 * 0.4) * 1000.0;
            strength[beat as usize] = 5.0;
        }
        let envelope = crate::onset::OnsetEnvelope::from_strength(0.001, strength);
        let mut raw = grid(0.5, 0.5, 8);
        // The second span's raw beats are 20 ms late and every 0.4 s, a real tempo change.
        for index in 0..8 {
            raw.push(RawBeat {
                time: 4.62 + index as f64 * 0.4,
                confidence: 0.9,
            });
        }
        let mut on = settings(9.0);
        on.refine = true;
        let analysis = build(raw, &[], Some(&envelope), &on).expect("builds");
        assert!(analysis.refinement.applied);
        for beat in analysis.beats.iter().take(8) {
            assert!(
                (beat.time - beat.raw).abs() < 0.001,
                "steady span stays put"
            );
        }
        let moved = analysis
            .beats
            .iter()
            .skip(8)
            .all(|beat| (beat.time - beat.raw + 0.02).abs() < 0.002);
        assert!(moved, "the late span follows its attacks");
    }

    #[test]
    fn refinement_never_increases_the_interval_spread() {
        // Several attack patterns and jitter sizes: the guarantee must hold for all of them.
        let patterns = [0.0, 0.008, 0.015, 0.025];
        let interference = [0.0, 0.02, -0.02];
        for offset in patterns {
            for vocal in interference {
                let mut strength = vec![0.0f32; 8_000];
                for index in 0..12 {
                    let beat = (0.5 + index as f64 * 0.5) * 1000.0;
                    strength[(beat as usize).saturating_sub(1)] = 1.0;
                    let other = beat + vocal * 1000.0;
                    if other > 0.0 {
                        let last = strength.len() - 1;
                        strength[(other as usize).min(last)] = 4.0;
                    }
                }
                let envelope = crate::onset::OnsetEnvelope::from_strength(0.001, strength);
                let offsets: Vec<f64> = (0..12)
                    .map(|index| if index % 2 == 0 { offset } else { -offset })
                    .collect();
                let raw = jittered(0.5, 0.5, &offsets, 0.9);
                let before: Vec<f64> = raw.iter().map(|beat| beat.time).collect();
                let mut on = settings(6.0);
                on.refine = true;
                let analysis = build(raw, &[], Some(&envelope), &on).expect("builds");
                assert!(
                    analysis.refinement.ioi_spread_after_ms
                        <= analysis.refinement.ioi_spread_before_ms + REFINE_SPREAD_SLACK_MS,
                    "offset {offset}, vocal {vocal}"
                );
                let after: Vec<f64> = analysis.beats.iter().map(|beat| beat.time).collect();
                assert!(spread_ms(&after) <= spread_ms(&before) + REFINE_SPREAD_SLACK_MS);
            }
        }
    }

    #[test]
    fn the_last_beat_of_a_segment_ignores_the_next_segment() {
        // Two spans, 0.6 s then 0.5 s, with the boundary right after the 0.6 s grid. The last beat
        // of the first span sits on its grid and its attack is 25 ms early. Averaged with the next
        // span's first beat (0.5 s away, not 0.6 s) the local line would lie 50 ms early and pull
        // the beat onto the attack; against its own span's line it is already where it belongs
        // and must stay.
        let mut strength = vec![0.0f32; 10_000];
        let mut beats = Vec::new();
        let mut push = |raw: f64, attack: f64| {
            strength[(attack * 1000.0).round() as usize] = 5.0;
            beats.push(Beat {
                raw,
                time: raw,
                confidence: 0.9,
                refined: false,
                is_downbeat: false,
                bar: None,
                beat_in_bar: None,
                segment: 0,
            });
        };
        for index in 0..7 {
            let truth = 0.5 + index as f64 * 0.6;
            let jitter = if index == 1 || index == 3 { 0.015 } else { 0.0 };
            let attack = if index == 6 { truth - 0.025 } else { truth };
            push(truth + jitter, attack);
        }
        for index in 0..8 {
            let time = 4.6 + index as f64 * 0.5;
            push(time, time);
        }
        let envelope = crate::onset::OnsetEnvelope::from_strength(0.001, strength);
        let times: Vec<f64> = beats.iter().map(|beat| beat.raw).collect();
        let steps = beat_steps(&times);
        let mut on = settings(9.5);
        on.refine = true;
        let mut warnings = Vec::new();
        let spans = [(0, 6), (7, 14)];
        let (refined, refinement) =
            refine_beats(beats, &spans, &steps, Some(&envelope), &on, &mut warnings);
        assert!(refinement.applied);
        assert!(refined[..=6].iter().all(|beat| beat.refined));
        let last = &refined[6];
        assert!(
            (last.time - last.raw).abs() < 0.002,
            "moved by {} ms",
            (last.time - last.raw) * 1000.0
        );
    }

    #[test]
    fn the_confidence_terms_are_readable() {
        // A perfect fit with a mediocre model is still a confident segment.
        let residuals = vec![5.0; 32];
        let confidence = segment_confidence(0.5, &residuals);
        assert!(confidence >= 0.8, "confidence {confidence}");
        // A badly tracked span falls, but the model term alone never drives it to zero.
        let bad: Vec<f64> = (0..32).map(|index| 150.0 + index as f64).collect();
        assert!(segment_confidence(0.0, &bad) < 0.5);
        assert!(segment_confidence(1.0, &bad) > 0.0);
    }
}
