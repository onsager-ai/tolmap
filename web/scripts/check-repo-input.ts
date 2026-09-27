#!/usr/bin/env -S npx tsx
// Unit checks for Home's repository field (docs/UX.md §4.9, phase 6): the
// pure parsing and validation src/api/repoInput.ts does before anything
// ever reaches src/api/client.ts -- accepting "owner/repo" and a
// github.com URL, rejecting the router's reserved names (routes/reserved.ts)
// client-side so a doomed request never round-trips, and giving the plain-
// words messages the "Input validation" state (§4.9's States list) shows.
// This surface had no test at all before phase 6 -- SubmitRepoForm.tsx is
// the only caller, and it's exercised in the browser only through
// check-view-stability.mjs's Home checks, never against its edge cases.
//
// No browser or test runner needed (web/README.md: none exists yet) -- same
// standalone-script pattern as check-search.ts and check-phone-shell.ts.
//
// Run: npx tsx web/scripts/check-repo-input.ts
import { parseRepoInput, validateRepoInput } from "../src/api/repoInput";
import { RESERVED_NAMES } from "../src/routes/reserved";

let failures = 0;
let checks = 0;

function report(ok: boolean, label: string, detail?: string) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${detail ? " -- " + detail : ""}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// -------------------------------------------------------------- parseRepoInput
console.log("\nparseRepoInput (owner/repo and github.com URLs)");
{
  report(eq(parseRepoInput("django/django"), { owner: "django", repo: "django", apiValue: "django/django" }), "a bare slug");
  report(eq(parseRepoInput("  django/django  "), { owner: "django", repo: "django", apiValue: "django/django" }), "surrounding whitespace is trimmed");
  report(
    eq(parseRepoInput("https://github.com/langgenius/dify"), { owner: "langgenius", repo: "dify", apiValue: "https://github.com/langgenius/dify" }),
    "a github.com URL keeps the pasted URL as apiValue, not a normalised slug",
  );
  report(
    eq(parseRepoInput("https://github.com/langgenius/dify.git"), { owner: "langgenius", repo: "dify", apiValue: "https://github.com/langgenius/dify.git" }),
    "a .git suffix is stripped from owner/repo but the pasted URL is untouched",
  );
  report(
    eq(parseRepoInput("http://www.github.com/onsager-ai/tolmap/"), { owner: "onsager-ai", repo: "tolmap", apiValue: "http://www.github.com/onsager-ai/tolmap/" }),
    "http, www, and a trailing slash are all accepted",
  );
  report(eq(parseRepoInput("django/django.git"), { owner: "django", repo: "django", apiValue: "django/django" }), "a bare slug's .git suffix is stripped and re-normalised");
  report(parseRepoInput("") === null, "empty input parses to nothing");
  report(parseRepoInput("   ") === null, "whitespace-only input parses to nothing");
  report(parseRepoInput("just-one-segment") === null, "a single segment (no owner) parses to nothing");
  report(parseRepoInput("owner/repo/extra") === null, "a third path segment parses to nothing");
  report(parseRepoInput("https://gitlab.com/owner/repo") === null, "a non-github.com host parses to nothing");
  report(parseRepoInput("-leading-dash/repo") === null, "an owner starting with a dash is invalid (GitHub's own rule)");
}

// -------------------------------------------------------------- validateRepoInput
console.log("\nvalidateRepoInput (§4.9's 'Input validation messages in plain words')");
{
  const ok = validateRepoInput("django/django");
  report(ok.ok === true, "a valid slug validates");

  const empty = validateRepoInput("   ");
  report(empty.ok === false && !!empty.message && /owner\/name|github\.com/.test(empty.message), "an unparseable input names the expected shape in plain words", JSON.stringify(empty));
  report(!/error|invalid_request|\{|\}/i.test(empty.ok ? "" : empty.message), "the message is prose, not an error code or the wire shape");

  for (const reserved of RESERVED_NAMES) {
    const r = validateRepoInput(`${reserved}/anything`);
    report(r.ok === false && r.message.includes(reserved), `"${reserved}" is refused client-side as a repository owner (routes/reserved.ts)`, JSON.stringify(r));
  }
  report(validateRepoInput(`${[...RESERVED_NAMES][0].toUpperCase()}/anything`).ok === false, "reserved-name matching is case-insensitive");

  const notReserved = validateRepoInput("settingsx/repo");
  report(notReserved.ok === true, "a name that only starts with a reserved word is not itself reserved", JSON.stringify(notReserved));
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
