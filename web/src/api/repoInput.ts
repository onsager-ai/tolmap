// Client-side shape validation for the submit box (milestone brief, "A
// submit flow"). Mirrors the contract's `{"repo": "owner/name" | "<https
// url>"}` — this never builds the `{"path": ...}` variant, which is
// service-local only. Kept separate from src/api/client.ts so the pure
// parsing logic is trivial to unit-drive without touching fetch.
import { RESERVED_NAMES } from "@/routes/reserved";

export interface ParsedRepo {
  owner: string;
  repo: string;
  /** Exactly what to send as the `repo` field on POST /api/index — the
   * original URL when one was pasted, otherwise the normalised `owner/name`. */
  apiValue: string;
}

const OWNER_RE = "[A-Za-z0-9](?:[A-Za-z0-9-]{0,38}[A-Za-z0-9])?";
const REPO_RE = "[A-Za-z0-9_.-]+";
const GH_URL_RE = new RegExp(
  `^https?://(?:www\\.)?github\\.com/(${OWNER_RE})/(${REPO_RE}?)(?:\\.git)?/?(?:[?#].*)?$`,
);
const SLUG_RE = new RegExp(`^(${OWNER_RE})/(${REPO_RE})$`);

export function parseRepoInput(raw: string): ParsedRepo | null {
  const value = raw.trim();
  if (!value) return null;

  const urlMatch = value.match(GH_URL_RE);
  if (urlMatch) {
    const [, owner, repoRaw] = urlMatch;
    const repo = repoRaw.replace(/\.git$/, "");
    if (!owner || !repo) return null;
    return { owner, repo, apiValue: value };
  }

  const slugMatch = value.match(SLUG_RE);
  if (slugMatch) {
    const [, owner, repoRaw] = slugMatch;
    const repo = repoRaw.replace(/\.git$/, "");
    if (!repo) return null;
    return { owner, repo, apiValue: `${owner}/${repo}` };
  }

  return null;
}

export type RepoInputValidation =
  | { ok: true; parsed: ParsedRepo }
  | { ok: false; message: string };

/** Also enforces the router's reserved names client-side (CLAUDE.md /
 * ARCHITECTURE.md: the top-level names must never resolve as an owner) —
 * the service will refuse it too, but there is no reason to round-trip a
 * request that can never succeed. */
export function validateRepoInput(raw: string): RepoInputValidation {
  const parsed = parseRepoInput(raw);
  if (!parsed) {
    return { ok: false, message: "enter owner/name or a github.com/owner/name URL" };
  }
  if (RESERVED_NAMES.has(parsed.owner.toLowerCase())) {
    return {
      ok: false,
      message: `"${parsed.owner}" is reserved by tolmap and can never be a repository owner`,
    };
  }
  return { ok: true, parsed };
}
