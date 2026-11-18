const RATED = ["exam", "drill"];

type Turn = { mode: string; score: number };

export function rate(turn: Turn) {
  return RATED.includes(turn.mode) ? turn.score : 0;
}

export function review(turns: Turn[]) {
  let total = 0;
  for (const t of turns) {
    total += rate(t);
  }
  return total;
}
