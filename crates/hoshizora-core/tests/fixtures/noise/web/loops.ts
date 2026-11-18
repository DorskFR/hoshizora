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
