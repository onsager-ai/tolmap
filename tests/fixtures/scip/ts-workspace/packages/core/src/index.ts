import invariant from "tiny-invariant";
import { clamp, lerp } from "./math";

export function checkedClamp(value: number, low: number, high: number): number {
  invariant(low <= high, "low must not exceed high");
  return clamp(value, low, high);
}

export function blend(from: number, to: number, share: number): number {
  return lerp(from, to, checkedClamp(share, 0, 1));
}
