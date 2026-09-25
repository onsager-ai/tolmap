export function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), high);
}

export function lerp(from: number, to: number, share: number): number {
  return from + (to - from) * share;
}
