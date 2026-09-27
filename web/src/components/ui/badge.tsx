import * as React from "react";
import { cn } from "@/lib/utils";

export function Badge({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      className={cn(
        "inline-block rounded-[6px] px-1.5 py-0.5 text-label uppercase",
        className,
      )}
      {...props}
    />
  );
}
