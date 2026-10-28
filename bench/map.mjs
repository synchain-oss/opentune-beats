// Shared reading of a tempo map. The generator renders the drums on these beats, the scorer
// compares the analysis with them, so both must agree on where a beat is and which beats are
// expected to carry percussion.
//
// Schema of the maps in bench/maps (only `segments` is required):
//
//   {
//     "schema": 1,
//     "name": "steady 100 BPM with two 3-bar gaps",   // shown by score.mjs
//     "sampleRate": 44100,
//     "beatsPerBar": 4,
//     "leadInSec": 0.5,                                // silence before the first beat
//     "segments": [{ "bpm": 100, "bars": 16 }],        // constant-tempo spans, in play order
//     "gaps": [{ "startBar": 5, "bars": 3 }],          // optional, see below
//     "vocalOnsets": true,                             // optional, see below
//     "acceptance": { "beats": true, "tempoMap": true } // optional, defaults to beats only
//   }
//
// `gaps` are 1-based bars counted from the first bar of the beat grid (the beat at `leadInSec`).
// A gap starts on the first beat of `startBar` and ends on the first beat after its `bars` bars,
// end exclusive: the beat at the end is the downbeat that starts the music again. With the default
// four beats per bar, `{ "startBar": 5, "bars": 3 }` covers bars 5, 6 and 7. The beat grid keeps
// advancing through a gap; the generator simply renders no drum, hat, bass or vocal attack there,
// only a sustained pad. The scorer drops the truth beats inside the gap, because no percussion is
// there to be detected.
//
// `vocalOnsets: true` renders short pitched notes between the beats (never on a beat). The exact
// pattern is fixed in gen.mjs so the same map always produces the same samples.
//
// `acceptance` says which score gates apply to the material. `beats` enables the existing
// +/-70 ms F-measure gate and defaults to true for older maps; `tempoMap` enables the tempo-map
// gates and defaults to false, so bench/tempo-map.json keeps its beat-only gate.

export function beatGrid(map) {
  if (!Array.isArray(map.segments) || map.segments.length === 0) {
    throw new Error('tempo map has no segments');
  }
  const beatsPerBar = map.beatsPerBar ?? 4;
  if (!Number.isInteger(beatsPerBar) || beatsPerBar < 1) {
    throw new Error(`invalid beatsPerBar ${map.beatsPerBar}`);
  }
  const leadIn = map.leadInSec ?? 0.5;
  if (!Number.isFinite(leadIn) || leadIn < 0) throw new Error(`invalid leadInSec ${map.leadInSec}`);

  const beats = [];
  let time = leadIn;
  let first = true;
  let bar = 1;
  let beatInBar = 1;
  let lastPeriod = 0;
  for (let segmentIndex = 0; segmentIndex < map.segments.length; segmentIndex += 1) {
    const segment = map.segments[segmentIndex];
    if (!Number.isFinite(segment.bpm) || segment.bpm <= 0) {
      throw new Error(`segment ${segmentIndex} has an invalid bpm`);
    }
    if (!Number.isInteger(segment.bars) || segment.bars < 1) {
      throw new Error(`segment ${segmentIndex} has an invalid bar count`);
    }
    const period = 60 / segment.bpm;
    lastPeriod = period;
    const count = segment.bars * beatsPerBar;
    for (let index = 0; index < count; index += 1) {
      if (!first) time += period;
      beats.push({
        timeSec: time,
        bpm: segment.bpm,
        periodSec: period,
        segment: segmentIndex,
        bar,
        beatInBar,
      });
      first = false;
      beatInBar += 1;
      if (beatInBar > beatsPerBar) {
        beatInBar = 1;
        bar += 1;
      }
    }
  }
  return { beats, beatsPerBar, endSec: time + lastPeriod };
}

export function beatTimes(map) {
  const grid = beatGrid(map);
  return { times: grid.beats.map((beat) => beat.timeSec), endSec: grid.endSec };
}

// Truth spans of the map's tempo segments: a segment starts on its first beat and ends where the
// next segment starts; the last one ends with the audio. Used by the scorer to look up the true
// tempo at a reported segment's midpoint.
export function segmentSpans(map) {
  const grid = beatGrid(map);
  let beatIndex = 0;
  return map.segments.map((segment) => {
    const startSec = grid.beats[beatIndex].timeSec;
    beatIndex += segment.bars * grid.beatsPerBar;
    const endSec = beatIndex < grid.beats.length ? grid.beats[beatIndex].timeSec : grid.endSec;
    return { bpm: segment.bpm, startSec, endSec };
  });
}

// Gap spans in seconds. The end is exclusive: the beat at endSec is the first beat after the gap.
export function gapSpans(map) {
  const specs = map.gaps ?? [];
  if (!Array.isArray(specs)) throw new Error('invalid gaps: expected an array');
  if (specs.length === 0) return [];
  const grid = beatGrid(map);
  const spans = [];
  for (const spec of specs) {
    if (!Number.isInteger(spec.startBar) || spec.startBar < 1) {
      throw new Error(`gap has an invalid startBar ${spec.startBar}`);
    }
    if (!Number.isInteger(spec.bars) || spec.bars < 1) {
      throw new Error(`gap at bar ${spec.startBar} has an invalid bar count`);
    }
    const startIndex = (spec.startBar - 1) * grid.beatsPerBar;
    const endIndex = startIndex + spec.bars * grid.beatsPerBar;
    if (startIndex >= grid.beats.length) {
      throw new Error(`gap starts at bar ${spec.startBar}, past the last bar`);
    }
    if (endIndex > grid.beats.length) {
      throw new Error(`gap at bar ${spec.startBar} ends past the last bar`);
    }
    const startSec = grid.beats[startIndex].timeSec;
    const endSec = endIndex < grid.beats.length ? grid.beats[endIndex].timeSec : grid.endSec;
    spans.push({ startBar: spec.startBar, bars: spec.bars, startSec, endSec });
  }
  spans.sort((left, right) => left.startSec - right.startSec);
  for (let index = 1; index < spans.length; index += 1) {
    if (spans[index].startSec < spans[index - 1].endSec - 1e-9) {
      throw new Error(`gaps at bars ${spans[index - 1].startBar} and ${spans[index].startBar} overlap`);
    }
  }
  return spans;
}

// True when timeSec falls in a gap. The span end is exclusive, so the downbeat that resumes the
// drums is outside the gap and is expected in the analysis.
export function isInGap(spans, timeSec) {
  return spans.some((span) => timeSec >= span.startSec && timeSec < span.endSec);
}
