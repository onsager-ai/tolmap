import * as React from "react";
import { cn } from "@/lib/utils";

export function Badge({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      className={cn(
        "inline-block rounded px-1 py-0.5 text-[9px] uppercase tracking-wide",
        className,
      )}
      {...props}
    />
  );
}
