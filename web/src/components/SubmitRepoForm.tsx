import { useState, type FormEvent } from "react";
import { useNavigate } from "@tanstack/react-router";
import { validateRepoInput } from "@/api/repoInput";
import { postIndexJob, ApiRequestError } from "@/api/client";
import { useServiceAvailable } from "@/data/queries";

/** Home's "Map a codebase" entry point (docs/UX.md §4.9): a labelled 16px
 * mono field and a full-width "Map it" button, stacked (never side by side
 * -- the approved artboard has no room for both at 16px on a 320px phone).
 * Validates the pasted shape client-side (src/api/repoInput.ts, which also
 * rejects the router's reserved names as an owner), POSTs it, and moves to
 * the progress view at /new. When the service isn't reachable the box stays
 * visible but inert -- the mapped-repositories list below it still works
 * either way. */
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
        setSubmitError({
          message: err.code === "busy" ? "The index queue is full. Please try again shortly." : err.message,
          tooLarge: err.tooLarge,
        });
      } else {
        setSubmitError({ message: err instanceof Error ? err.message : String(err), tooLarge: false });
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form onSubmit={(e) => void onSubmit(e)} className="flex flex-col gap-2">
      <label htmlFor="repo-field" className="text-small font-semibold">
        GitHub repository
      </label>
      <input
        id="repo-field"
        data-repo-field
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          if (validationError) setValidationError(null);
        }}
        placeholder="owner/repo or a github.com link"
        aria-invalid={validationError != null}
        aria-describedby={validationError ? "repo-field-error" : undefined}
        disabled={disabled || submitting}
        className="h-[52px] rounded-[14px] border border-[var(--rule)] bg-[var(--chrome2)] px-4 font-mono text-body text-[var(--on)] outline-none placeholder:text-[var(--dim)] focus-visible:ring-2 focus-visible:ring-[var(--accent)] disabled:opacity-50"
      />
      <button
        type="submit"
        disabled={disabled || submitting || value.trim().length === 0}
        className="h-[52px] rounded-[14px] bg-[var(--accent)] text-body font-bold text-[var(--on-accent)] disabled:opacity-50"
      >
        {submitting ? "mapping…" : "Map it"}
      </button>
      <p className="text-meta text-[var(--dim)]">
        A large repository can take several minutes; you can leave and come back.
      </p>
      {disabled && (
        <p className="text-meta text-[var(--dim)]">
          the indexing service isn't reachable right now — the mapped repositories below still work.
        </p>
      )}
      {validationError && (
        <p id="repo-field-error" data-repo-validation-error className="text-meta text-[var(--link-out)]">
          {validationError}
        </p>
      )}
      {submitError && (
        <p data-repo-submit-error className={`text-meta ${submitError.tooLarge ? "text-[var(--warn)]" : "text-[var(--link-out)]"}`}>
          {submitError.tooLarge ? "too large for the hosted index: " : ""}
          {submitError.message}
        </p>
      )}
    </form>
  );
}
