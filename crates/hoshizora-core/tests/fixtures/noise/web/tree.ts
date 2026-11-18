type Tree = { left?: Tree; right?: Tree };

export function height(node?: Tree): number {
  if (!node) return 0;
  return Math.max(height(node.left), height(node.right)) + 1;
}

export function fib(n: number): number {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}

declare function lazy(f: () => unknown): unknown;
declare function list(item: unknown): unknown;

export function condition(): unknown {
  return lazy(() => ({ not: condition, all: list(condition) }));
}
