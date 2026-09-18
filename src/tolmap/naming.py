"""Stage 04 — district naming.

Naming is the one stage that wants a language model: given the central files of a
cluster, produce the name a person on the team would use. Two things matter more
than the model choice:

1. The name must be CACHED against a fingerprint of the cluster's membership.
   An uncached naming step renames `payments` to `billing` on a rerun and
   invalidates every spatial memory the team has built.
2. There must be a deterministic fallback, so the tool is useful with no model
   and no network. The fallback below derives a name from the dominant
   directories, which is right often enough to be worth shipping.
"""
import hashlib
import json
import os
from collections import Counter

STOP = {"src", "lib", "pkg", "internal", "packages", "core", "app", "python"}


def fingerprint(members):
    h = hashlib.sha1("\n".join(sorted(members)).encode()).hexdigest()
    return h[:12]


def segment_df(all_files):
    """How many files each path segment appears in — the package root appears in
    all of them and must not win every district."""
    df = Counter()
    for f in all_files:
        for p in set(f.split("/")[:-1]):
            df[p] += 1
    return df, len(all_files)


def auto_name(members, df=None, total=0):
    """Deterministic fallback: the directories that DISTINGUISH the cluster.

    Weighted by inverse document frequency, so `httpie/` — which every file in
    the httpie repo shares — scores near zero and the distinctive segment wins.
    """
    import math
    segs = Counter()
    for f in members:
        parts = [p for p in f.split("/")[:-1] if p not in STOP]
        for depth, p in enumerate(parts):
            # A segment carried by most of the repo (the package root) names
            # nothing. Drop it outright rather than flooring its weight.
            if df and total and df[p] > total * 0.6:
                continue
            idf = math.log((total + 1) / (df[p] + 1)) if df and total else 1.0
            segs[p] += (1.0 / (1 + depth * 0.4)) * idf
    segs = Counter({k: v for k, v in segs.items() if v > 0.08})
    if segs:
        top = [w for w, _ in segs.most_common(2)]
        if len(top) > 1 and segs[top[1]] > segs[top[0]] * 0.55:
            return f"{top[0]} & {top[1]}"
        return top[0]
    # A flat package (every file in one directory) leaves the path with nothing
    # to say. Fall back to the cluster's own filenames, which at least differ.
    return filename_name(members)


def filename_name(members, nodes=None):
    stems = []
    for f in members:
        stem = f.split("/")[-1].rsplit(".", 1)[0].lstrip("_")
        if stem and stem not in ("init", "main", "index", "base", "common"):
            stems.append(stem)
    if not stems:
        return "misc"
    # prefer the longest-lived shared word, else the two shortest names, which
    # in practice are the core modules the rest hang off
    words = Counter()
    for s_ in stems:
        for w in s_.replace("-", "_").split("_"):
            if len(w) > 3:
                words[w] += 1
    common = [w for w, c in words.most_common(2) if c >= max(2, len(stems) * 0.3)]
    if common:
        return " & ".join(common[:2])
    stems.sort(key=lambda x: (len(x), x))
    return " & ".join(stems[:2])


def central_files(members, nodes, limit=12):
    """The files a namer should actually look at: the biggest and most depended on."""
    scored = sorted(members,
                    key=lambda f: -(nodes[f]["fanin"] * 3 + nodes[f]["loc"] / 60))
    return scored[:limit]


def load_cache(path):
    if path and os.path.exists(path):
        return json.load(open(path))
    return {}


def save_cache(path, cache):
    if path:
        json.dump(cache, open(path, "w"), indent=1, sort_keys=True)


def name_districts(by_district, nodes, cache_path=None, namer=None):
    """Return {district_id: name}.

    `namer(context) -> str` is the model hook. `context` carries the previous
    name so the model RENAMES rather than re-invents — drift is the failure mode
    that matters here, not name quality.
    """
    cache = load_cache(cache_path)
    all_files = [f for ms in by_district.values() for f in ms]
    df, total = segment_df(all_files)
    out, used = {}, set()
    for d, members in sorted(by_district.items(), key=lambda kv: -len(kv[1])):
        key = fingerprint(members)
        hit = cache.get(key)
        if hit:
            out[d] = hit["name"]
            used.add(hit["name"])
            continue
        prev = None
        for v in cache.values():
            if v.get("district") == d:
                prev = v["name"]
                break
        ctx = {"district": d, "size": len(members),
               "files": central_files(members, nodes), "previous_name": prev}
        nm = None
        if namer:
            try:
                nm = namer(ctx)
            except Exception:
                nm = None
        nm = (nm or auto_name(members, df, total)).strip()[:32]
        if nm in used:                       # two districts must never share a name
            nm = f"{nm} {sum(1 for u in used if u.startswith(nm)) + 1}"
        used.add(nm)
        out[d] = nm
        cache[key] = {"name": nm, "district": d, "size": len(members)}
    save_cache(cache_path, cache)
    return out


def naming_prompt(ctx):
    """The prompt a model hook should send. Kept here so the contract is visible
    and testable without a network call."""
    lines = "\n".join(f"  {f}" for f in ctx["files"])
    prev = (f"\nIt was previously called '{ctx['previous_name']}'. Keep that name "
            f"unless the membership has clearly changed meaning."
            if ctx.get("previous_name") else "")
    return (
        f"These {ctx['size']} files were clustered together by their imports, "
        f"co-change history and vocabulary. Its most central members are:\n"
        f"{lines}\n{prev}\n\n"
        "Give this group the short lowercase name a developer on this team would "
        "use for the concern it represents. Two or three words at most. Name the "
        "concern, not the folder. Reply with the name only."
    )
