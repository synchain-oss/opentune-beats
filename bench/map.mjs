// Shared reading of a tempo map. The generator renders the drum loop on these beats and the scorer
// compares the analysis with them, so both must agree on where a beat is.

export function beatTimes(map) {
  const beatsPerBar = map.beatsPerBar ?? 4;
  const leadIn = map.leadInSec ?? 0.5;
  const times = [];
  let time = leadIn;
  let first = true;
  for (const segment of map.segments) {
    const period = 60 / segment.bpm;
    for (let index = 0; index < segment.bars * beatsPerBar; index += 1) {
      if (!first) time += period;
      times.push(time);
      first = false;
    }
  }
  const lastPeriod = 60 / map.segments[map.segments.length - 1].bpm;
  return { times, endSec: time + lastPeriod };
}
