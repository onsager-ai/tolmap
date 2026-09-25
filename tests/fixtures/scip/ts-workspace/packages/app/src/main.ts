import { blend, checkedClamp } from "@fixture/core";
import { percent } from "./format";

export function progress(done: number, total: number): string {
  return percent(checkedClamp(done / total, 0, 1));
}

export function midpoint(from: number, to: number): number {
  return blend(from, to, 0.5);
}
