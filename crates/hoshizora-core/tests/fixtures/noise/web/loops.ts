export function publicDrops(steps: { expect: string[] }[]) {
  const out: string[] = [];
  for (const step of steps) {
    for (const e of step.expect) {
      out.push(e);
    }
  }
  return out;
}

export function interleave(xs: number[]) {
  let n = 0;
  for (let pass = 0; pass < 3; pass++) {
    for (const x of xs) {
      for (const y of xs) {
        n += x * y * pass;
      }
    }
  }
  return n;
}

type Clip = { bytes: number[] };
type Decoded = { length: number; channels: number; at(c: number, i: number): number };
declare function decode(clip: Clip): Decoded;

export function mixdown(clips: Clip[]) {
  const out: number[][] = [];
  for (const clip of clips) {
    const buffer = decode(clip);
    const mono = new Array(buffer.length).fill(0);
    for (let c = 0; c < buffer.channels; c++) {
      for (let i = 0; i < mono.length; i++) mono[i] += buffer.at(c, i);
    }
    out.push(mono);
  }
  return out;
}

const GAP = 3;

export function nearHits(words: string[], keys: string[]) {
  let hits = 0;
  for (let at = 0; at < words.length; at++) {
    const limit = Math.min(words.length - 1, at + GAP);
    for (const k of keys) {
      for (let i = at; i <= limit; i++) if (words[i] === k) hits++;
    }
  }
  return hits;
}
