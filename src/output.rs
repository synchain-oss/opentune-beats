//! The JSON documents: the analysis result and `version --json`.

use crate::json::Json;
use crate::models;
use crate::post;
use crate::{NAME, SCHEMA_VERSION, VERSION};

/// Everything the analysis document needs besides the post-processed grid.
pub struct Document<'a> {
    pub model: &'a models::ModelFile,
    pub file_name: &'a str,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub refine: bool,
    pub max_refine_ms: f64,
    pub min_segment_beats: usize,
    pub frame_sec: f64,
    pub analysis: &'a post::Analysis,
}

/// The `analyze` document of `docs/output.md`, section 2.
pub fn document(context: &Document<'_>) -> Json {
    let analysis = context.analysis;
    let beats: Vec<Json> = analysis
        .beats
        .iter()
        .enumerate()
        .map(|(index, beat)| beat_json(index, beat))
        .collect();
    let segments: Vec<Json> = analysis.segments.iter().map(segment_json).collect();
    let changes: Vec<Json> = analysis.tempo_changes.iter().map(change_json).collect();
    let warnings: Vec<Json> = analysis.warnings.iter().map(warning_json).collect();
    let low: Vec<Json> = analysis
        .segments
        .iter()
        .filter(|segment| segment.confidence < 0.5)
        .map(|segment| Json::count(segment.index))
        .collect();
    let mut downbeats = 0;
    for beat in &analysis.beats {
        if beat.is_downbeat {
            downbeats += 1;
        }
    }

    let model = Json::object(vec![
        ("name", Json::text(context.model.key)),
        ("file", Json::text(context.model.file)),
        ("sha256", Json::text(context.model.sha256)),
        ("melSha256", Json::text(models::MEL.sha256)),
    ]);
    let input = Json::object(vec![
        ("fileName", Json::text(context.file_name)),
        ("durationSec", Json::fixed(context.duration, 4)),
        ("sampleRateHz", Json::unsigned(context.sample_rate)),
        ("channels", Json::unsigned(context.channels as u32)),
    ]);
    let params = Json::object(vec![
        ("refine", Json::Bool(context.refine)),
        ("maxRefineMs", Json::fixed(context.max_refine_ms, 0)),
        ("minSegmentBeats", Json::count(context.min_segment_beats)),
        ("frameSec", Json::fixed(context.frame_sec, 3)),
    ]);
    let refinement = refinement_json(&analysis.refinement);
    let summary = Json::object(vec![
        ("beats", Json::count(analysis.beats.len())),
        ("downbeats", Json::count(downbeats)),
        ("segments", Json::count(analysis.segments.len())),
        ("dominantBpm", Json::fixed(analysis.dominant_bpm, 3)),
        ("lowConfidenceSegments", Json::array(low)),
    ]);

    Json::object(vec![
        ("schemaVersion", Json::count(SCHEMA_VERSION as usize)),
        ("tool", tool_json()),
        ("model", model),
        ("input", input),
        ("params", params),
        ("beats", Json::array(beats)),
        ("segments", Json::array(segments)),
        ("tempoChanges", Json::array(changes)),
        ("refinement", refinement),
        ("summary", summary),
        ("warnings", Json::array(warnings)),
    ])
}

/// The `version --json` document of `docs/output.md`, section 4.
pub fn version_document() -> Json {
    let models_json = Json::object(vec![
        ("final", model_json(&models::FINAL)),
        ("small", model_json(&models::SMALL)),
        ("mel", model_json(&models::MEL)),
    ]);
    Json::object(vec![
        ("name", Json::text(NAME)),
        ("version", Json::text(VERSION)),
        ("beatThis", Json::text(models::BEAT_THIS_VERSION)),
        ("schemaVersion", Json::count(SCHEMA_VERSION as usize)),
        ("models", models_json),
    ])
}

fn tool_json() -> Json {
    Json::object(vec![
        ("name", Json::text(NAME)),
        ("version", Json::text(VERSION)),
        ("beatThis", Json::text(models::BEAT_THIS_VERSION)),
    ])
}

fn model_json(entry: &models::ModelFile) -> Json {
    Json::object(vec![
        ("file", Json::text(entry.file)),
        ("sha256", Json::text(entry.sha256)),
    ])
}

fn beat_json(index: usize, beat: &post::Beat) -> Json {
    Json::object(vec![
        ("index", Json::count(index)),
        ("timeSec", Json::fixed(beat.time, 4)),
        ("rawTimeSec", Json::fixed(beat.raw, 4)),
        ("confidence", Json::fixed(beat.confidence, 4)),
        ("isDownbeat", Json::Bool(beat.is_downbeat)),
        ("bar", optional_u32(beat.bar)),
        ("beatInBar", optional_u32(beat.beat_in_bar)),
        ("segment", Json::count(beat.segment)),
    ])
}

fn change_json(change: &post::TempoChange) -> Json {
    Json::object(vec![
        ("atSec", Json::fixed(change.at_sec, 4)),
        ("fromBpm", Json::fixed(change.from_bpm, 3)),
        ("toBpm", Json::fixed(change.to_bpm, 3)),
        ("kind", Json::text(change.kind)),
    ])
}

fn warning_json(entry: &post::Warning) -> Json {
    Json::object(vec![
        ("code", Json::text(entry.code)),
        ("detail", Json::text(entry.detail.as_str())),
        ("segment", optional_count(entry.segment)),
    ])
}

fn refinement_json(refinement: &post::Refinement) -> Json {
    Json::object(vec![
        ("applied", Json::Bool(refinement.applied)),
        (
            "globalOffsetMs",
            Json::fixed(refinement.global_offset_ms, 3),
        ),
        (
            "medianAdjustMs",
            Json::fixed(refinement.median_adjust_ms, 3),
        ),
        ("p95AdjustMs", Json::fixed(refinement.p95_adjust_ms, 3)),
    ])
}

fn segment_json(segment: &post::Segment) -> Json {
    let candidates: Vec<Json> = segment
        .candidates
        .iter()
        .map(|candidate| {
            Json::object(vec![
                ("numerator", Json::count(candidate.0 as usize)),
                ("denominator", Json::count(candidate.1 as usize)),
                ("score", Json::fixed(candidate.2, 4)),
            ])
        })
        .collect();
    let signature = Json::object(vec![
        ("numerator", Json::count(segment.signature.0 as usize)),
        ("denominator", Json::count(segment.signature.1 as usize)),
    ]);
    let residuals = Json::object(vec![
        ("median", Json::fixed(segment.residual_median_ms, 2)),
        ("p95", Json::fixed(segment.residual_p95_ms, 2)),
    ]);
    let octave = match segment.octave_note {
        Some(note) => Json::text(note),
        None => Json::null(),
    };
    Json::object(vec![
        ("index", Json::count(segment.index)),
        ("startSec", Json::fixed(segment.start, 4)),
        ("endSec", Json::fixed(segment.end, 4)),
        ("firstBeat", Json::count(segment.first_beat)),
        ("lastBeat", Json::count(segment.last_beat)),
        ("bpm", Json::fixed(segment.bpm, 3)),
        ("beatPeriodSec", Json::fixed(segment.period, 6)),
        ("timeSignature", signature),
        ("timeSignatureCandidates", Json::array(candidates)),
        ("confidence", Json::fixed(segment.confidence, 4)),
        ("fitResidualMs", residuals),
        ("octaveNote", octave),
    ])
}

fn optional_u32(value: Option<u32>) -> Json {
    match value {
        Some(value) => Json::count(value as usize),
        None => Json::null(),
    }
}

fn optional_count(value: Option<usize>) -> Json {
    match value {
        Some(value) => Json::count(value),
        None => Json::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::post::{Analysis, Beat, Refinement, Segment};

    fn empty_analysis() -> Analysis {
        Analysis {
            beats: Vec::new(),
            segments: Vec::new(),
            tempo_changes: Vec::new(),
            refinement: Refinement {
                applied: false,
                global_offset_ms: 0.0,
                median_adjust_ms: 0.0,
                p95_adjust_ms: 0.0,
            },
            warnings: Vec::new(),
            dominant_bpm: 0.0,
        }
    }

    #[test]
    fn the_version_document_lists_every_model() {
        let text = version_document().to_compact();
        assert!(text.contains("\"beatThis\":\"1.1.0\""));
        assert!(text.contains("\"final\":{\"file\":\"beat_this_final.onnx\""));
        assert!(text.contains("\"small\":{\"file\":\"beat_this_small.onnx\""));
        assert!(text.contains("\"mel\":{\"file\":\"mel_spectrogram.onnx\""));
    }

    #[test]
    fn the_analysis_document_uses_the_contract_keys() {
        let mut analysis = empty_analysis();
        analysis.beats.push(Beat {
            raw: 0.5,
            time: 0.5,
            confidence: 0.9,
            is_downbeat: true,
            bar: Some(1),
            beat_in_bar: Some(1),
            segment: 0,
        });
        analysis.segments.push(Segment {
            index: 0,
            start: 0.0,
            end: 8.0,
            first_beat: 0,
            last_beat: 0,
            bpm: 120.0,
            period: 0.5,
            signature: (4, 4),
            candidates: vec![(4, 4, 1.0)],
            confidence: 0.8,
            residual_median_ms: 1.0,
            residual_p95_ms: 2.0,
            octave_note: None,
        });
        let context = Document {
            model: &crate::models::FINAL,
            file_name: "take1.wav",
            duration: 8.0,
            sample_rate: 44_100,
            channels: 2,
            refine: true,
            max_refine_ms: 30.0,
            min_segment_beats: 8,
            frame_sec: 0.02,
            analysis: &analysis,
        };
        let text = document(&context).to_compact();
        assert!(text.contains("\"schemaVersion\":1"));
        assert!(text.contains("\"fileName\":\"take1.wav\""));
        assert!(text.contains("\"isDownbeat\":true"));
        assert!(text.contains("\"octaveNote\":null"));
        assert!(text.contains("\"dominantBpm\":0.000"));
        assert_eq!(text.lines().count(), 1);
    }
}
