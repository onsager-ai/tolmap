import { useState, type FormEvent } from "react";
import { useNavigate } from "@tanstack/react-router";
import { validateRepoInput } from "@/api/repoInput";
import { postIndexJob, ApiRequestError } from "@/api/client";
import { useServiceAvailable } from "@/data/queries";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

/** The catalogue route's "map a repository" entry point (milestone brief,
 * "A submit flow" + "The catalogue becomes a real index page"). Validates
 * the pasted shape client-side (src/api/repoInput.ts, which also rejects
 * the router's reserved names as an owner), POSTs it, and moves to the
 * progress view at /new. When the service isn't reachable the box stays
 * visible but inert — the bundled catalogue below it still works either way. */
export function SubmitRepoForm() {
  const navigate = useNavigate();
  const { data: serviceAvailable } = useServiceAvailable();
  const [value, setValue] = useState("");
  const [validationError, setValidationError] = useState<string | null>(null);
  const [submitError, setSubmitError] = useState<{ message: string; tooLarge: boolean } | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const disabled = serviceAvailable === false;

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (disabled || submitting) return;
    setSubmitError(null);

    const result = validateRepoInput(value);
    if (!result.ok) {
      setValidationError(result.message);
      return;
    }
    setValidationError(null);
    setSubmitting(true);
    try {
      const res = await postIndexJob({ repo: result.parsed.apiValue });
      if (res.status === "done") {
        const [owner, repo] = res.slug.split("/");
        await navigate({ to: "/$owner/$repo", params: { owner, repo }, search: { geo: "r", layer: "d" } });
      } else {
        await navigate({ to: "/new", search: { job: res.job_id, slug: res.slug } });
      }
    } catch (err) {
      if (err instanceof ApiRequestError) {
        setSubmitError({ message: err.message, tooLarge: err.tooLarge });
      } else {
        setSubmitError({ message: err instanceof Error ? err.message : String(err), tooLarge: false });
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form onSubmit={(e) => void onSubmit(e)} className="mt-6 flex flex-col gap-2">
      <div className="flex flex-col gap-2 sm:flex-row">
        <Input
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            if (validationError) setValidationError(null);
          }}
          placeholder="owner/name or https://github.com/owner/name"
          aria-label="Repository to map"
          disabled={disabled || submitting}
          className="flex-1"
        />
        <Button type="submit" disabled={disabled || submitting || value.trim().length === 0}>
          {submitting ? "submitting…" : "map it"}
        </Button>
      </div>
      {disabled && (
        <p className="text-[11px] text-[var(--dim)]">
          the indexing service isn't reachable right now — browsing the bundled catalogue below still works.
        </p>
      )}
      {validationError && <p className="text-[11px] text-[var(--hot)]">{validationError}</p>}
      {submitError && (
        <p className={`text-[11px] ${submitError.tooLarge ? "text-[#8A6B1C]" : "text-[var(--hot)]"}`}>
          {submitError.tooLarge ? "too large for the hosted index: " : ""}
          {submitError.message}
        </p>
      )}
    </form>
  );
}
