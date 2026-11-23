"""Run the pre-registered #375 backtest on GitHub Actions after building tolmap."""

import argparse
from collections import Counter, defaultdict
from heapq import nsmallest
from itertools import combinations, product
import json
import math
import os
from pathlib import Path
import random
from statistics import quantiles
import subprocess
from tempfile import TemporaryDirectory
import tomllib


SEED = 7
PAST = 100
FUTURE = 50
CUTOFFS = 5
TOP = 10
RESAMPLES = 10_000
MIN_GAIN = 0.05
# Fixed before dispatch. Each component is scaled to its maximum at the cutoff.
WEIGHTS = {"fan_in": 1 / 3, "co_change": 1 / 3, "hidden_coupling": 1 / 3}
SEMON = {
    "url": "https://github.com/onsager-ai/semon.git",
    "commit": "fe77ddb157581d818efb5822b6520b5e063391aa",
    "parcels": True,
}


def git(repo, *args):
    return subprocess.check_output(["git", "-C", repo, *args])


def cutoff_indices(length):
    first, last = PAST, length - FUTURE - 1
    if last - first + 1 < CUTOFFS:
        return []
    return [first + i * (last - first) // (CUTOFFS - 1) for i in range(CUTOFFS)]


def changed_paths(repo, parent, commit):
    """Yield the status and old/new names from the first-parent tree diff."""
    fields = iter(git(repo, "diff", "--no-ext-diff", "--name-status", "-z",
                      "-M", "-l0", parent, commit, "--").decode(errors="surrogateescape")
                  .split("\0")[:-1])
    for status in fields:
        path = next(fields)
        yield status[0], path, next(fields) if status.startswith("R") else path


def touches(repo, commit_diffs, files, backward=False):
    """Return one set of cutoff file identities per commit, newest first if backward."""
    by_path = dict(zip(files, files))
    for parent, commit in commit_diffs:
        touched = set()
        moved = {}
        for status, old, new in changed_paths(repo, parent, commit):
            if backward:
                old, new = new, old
            identity = by_path.get(old)
            if identity is None:
                continue
            if (backward and status == "D") or (not backward and status == "A"):
                continue
            touched.add(identity)
            if status in "RAD":
                del by_path[old]
            if status == "R":
                moved[new] = identity
        by_path.update(moved)
        yield touched


def touch_masks(files, commits):
    masks = dict.fromkeys(files, 0)
    for i, changed in enumerate(commits):
        bit = 1 << i
        for file in changed:
            masks[file] |= bit
    return masks


def pair_groups(masks):
    """Yield groups of pairs sharing a positive co-change count.

    Collapsing identical touch masks avoids materializing all file pairs,
    including the usually large set with zero co-changes.
    """
    by_mask = defaultdict(list)
    for file, mask in masks.items():
        if mask:
            by_mask[mask].append(file)
    groups = sorted(by_mask.items())
    for i, (left_mask, left) in enumerate(groups):
        if len(left) > 1:
            yield left_mask.bit_count(), left, left, math.comb(len(left), 2)
        for j in range(i + 1, len(groups)):
            right_mask, right = groups[j]
            count = (left_mask & right_mask).bit_count()
            if count:
                yield count, left, right, len(left) * len(right)


def count_at_rank(histogram, rank):
    for count in sorted(histogram, reverse=True):
        rank -= histogram[count]
        if rank <= 0:
            return count
    return 0


def baseline_pairs(masks):
    histogram = Counter()
    for count, _, _, size in pair_groups(masks):
        histogram[count] += size
    boundary = count_at_rank(histogram, TOP)

    def positive_candidates():
        for count, left, right, _ in pair_groups(masks):
            if count >= boundary:
                # No group can contribute a top-ten pair with an endpoint
                # beyond its first ten paths in lexicographic order.
                pairs = (combinations(left[:TOP + 1], 2) if left is right
                         else product(left[:TOP], right[:TOP]))
                for a, b in pairs:
                    yield -count, tuple(sorted((a, b)))

    ranked = [pair for _, pair in nsmallest(TOP, positive_candidates())]
    if len(ranked) < TOP:
        for a, b in combinations(masks, 2):
            if not (masks[a] & masks[b]):
                ranked.append((a, b))
                if len(ranked) == TOP:
                    break
    return ranked


def truth_thresholds(masks):
    file_histogram = Counter(mask.bit_count() for mask in masks.values())
    pair_histogram = Counter()
    for count, _, _, size in pair_groups(masks):
        pair_histogram[count] += size
    # Nearest-rank top decile. Equal counts share the best rank in the tie,
    # including zero if the cutoff has too few positively touched candidates.
    return (count_at_rank(file_histogram, (len(masks) + 9) // 10),
            count_at_rank(pair_histogram, (math.comb(len(masks), 2) + 9) // 10))


def tolmap_ranks(document, debt, past, eligible):
    fan_in = {path: node[6] for path, node in zip(document["F"], document["N"])
              if path in eligible}
    # Match debt's god-file fan-in adjustment before its finding cutoff.
    for index, mass in (document.get("D") or {}).get("descendant_fanin", []):
        path = document["F"][index]
        if path in fan_in:
            fan_in[path] = math.floor(max(0, fan_in[path] - mass) * 100 + 0.5) / 100
    for row in debt["god_files"]:
        if row["path"] in fan_in:
            fan_in[row["path"]] = row["fanin"]

    co_change = Counter()
    for changed in past:
        mapped = changed.intersection(fan_in)
        for path in mapped:
            co_change[path] += len(mapped) - 1
    hidden_coupling = Counter()
    pairs = []
    for row in debt["hidden_coupling"]:
        a, b = row["left"], row["right"]
        if a in eligible and b in eligible:
            pairs.append(tuple(sorted((a, b))))
            for path in (a, b):
                hidden_coupling[path] += row["weight"]

    signals = {"fan_in": fan_in, "co_change": co_change, "hidden_coupling": hidden_coupling}
    maxima = {key: max(values.values(), default=0) for key, values in signals.items()}

    def score(path):
        return sum(WEIGHTS[key] * values.get(path, 0) / maxima[key]
                   for key, values in signals.items() if maxima[key])

    files = sorted(fan_in, key=lambda path: (-score(path), path))
    return files[:TOP], pairs[:TOP]


def evaluate_cutoff(binary, repo, name, spec, history, index, empty_tree, work):
    cutoff = history[index]
    print(f"{name}: cutoff {cutoff} ({index + 1}/{len(history)})", flush=True)
    git(repo, "checkout", "--quiet", "--detach", cutoff)
    records = git(repo, "ls-tree", "-r", "-z", cutoff).split(b"\0")[:-1]
    files = sorted(record.split(b"\t", 1)[1].decode(errors="surrogateescape")
                   for record in records if record.split(b" ", 2)[1] == b"blob")
    # Preserve the fixture's source root where it exists. Some earliest samples
    # predate a fixture's move to its current package directory.
    if "pkg" in spec:
        root = spec["pkg"] if (repo / spec["pkg"]).is_dir() else "."
        source = ["--pkg", root, "--lang", spec["lang"]]
    else:
        source = ["--all-sources"]
    build_out = work / cutoff
    command = [binary, "build", repo, *source, "--refs", "hand", "--name", name,
               "--out", build_out, "--resolution", "1.1"]
    if not spec["parcels"]:
        command.append("--no-parcels")
    subprocess.run(command, check=True, stdout=subprocess.DEVNULL)
    map_path = build_out / f"{name}.json"
    document = json.loads(map_path.read_text())
    debt = json.loads(subprocess.check_output([binary, "debt", "--map", map_path,
                                               "--format", "json"]))

    diffs = list(zip([empty_tree, *history], history))
    # The cutoff's own diff bridges any rename at T for the baseline. Its
    # touches are excluded, leaving precisely the 100 preceding commits.
    past = list(touches(repo, reversed(diffs[index - PAST:index + 1]), files, backward=True))[1:]
    past_masks = touch_masks(files, past)
    raw_files = sorted(files, key=lambda path: (-past_masks[path].bit_count(), path))[:TOP]
    raw_pairs = baseline_pairs(past_masks)
    seam_files, hidden_pairs = tolmap_ranks(document, debt, past, past_masks)

    future = touch_masks(files, touches(repo, diffs[index + 1:index + FUTURE + 1], files))
    file_threshold, pair_threshold = truth_thresholds(future)

    def score(baseline, tolmap, count, threshold):
        scored = {"threshold": threshold, "precision_at_10": {}}
        # Issue #375, Amendment 1: zero future touches never count as relevant.
        relevance_threshold = max(threshold, 1)
        for arm, ranking in (("baseline", baseline), ("tolmap", tolmap)):
            rows = []
            for value in ranking:
                future_count = count(value)
                rows.append({"candidate": value, "future_count": future_count,
                             "relevant": future_count >= relevance_threshold})
            scored[arm] = rows
            scored["precision_at_10"][arm] = sum(row["relevant"] for row in rows) / TOP
        return scored

    def file_count(path):
        return future[path].bit_count()

    def pair_count(pair):
        a, b = pair
        return (future[a] & future[b]).bit_count()

    return {
        "cutoff": cutoff,
        "first_parent_index": index,
        "past_first": history[index - PAST],
        "past_last": history[index - 1],
        "future_first": history[index + 1],
        "future_last": history[index + FUTURE],
        "source_args": source,
        "tracked_files": len(files),
        "mapped_files": len(document["F"]),
        "hidden_coupling_measured": debt["hidden_coupling_measured"],
        "files": score(raw_files, seam_files, file_count, file_threshold),
        "pairs": score(raw_pairs, hidden_pairs, pair_count, pair_threshold),
    }


def evaluate_repo(binary, name, spec, work):
    repo = work / name
    repo.mkdir()
    git(repo, "init", "--quiet")
    # Fetch exactly the pinned commit and its ancestry. No ref from a newer
    # checkout can affect the history, cutoff selection or map build.
    git(repo, "fetch", "--quiet", "--no-tags", spec["url"], spec["commit"])
    history = git(repo, "rev-list", "--first-parent", "--reverse", "FETCH_HEAD").decode().splitlines()
    if history[-1] != spec["commit"]:
        raise ValueError(f"{name}: fetched {history[-1]} instead of {spec['commit']}")
    indices = cutoff_indices(len(history))
    result = {"repo": name, "url": spec["url"], "pin": spec["commit"],
              "first_parent_commits": len(history), "cutoffs": []}
    if not indices:
        result["skip"] = f"fewer than {PAST + FUTURE + CUTOFFS} first-parent commits"
        print(f"{name}: skipped ({len(history)} commits)", flush=True)
        return result
    empty_tree = git(repo, "hash-object", "-w", "-t", "tree", "/dev/null").decode().strip()
    for index in indices:
        result["cutoffs"].append(evaluate_cutoff(binary, repo, name, spec, history,
                                                 index, empty_tree, work))
    return result


def aggregate(repos, signal):
    def hits(cutoff, arm):
        return sum(row["relevant"] for row in cutoff[signal][arm])

    active = [repo for repo in repos if repo["cutoffs"]]
    scores = [(hits(cutoff, "tolmap"), hits(cutoff, "baseline"))
              for repo in active for cutoff in repo["cutoffs"]]
    differences = [a - b for a, b in scores]
    if not differences:
        raise ValueError("no repository has five eligible cutoffs")
    divisor = TOP * len(scores)
    rng = random.Random(SEED)
    boot = [sum(rng.choices(differences, k=len(scores))) for _ in range(RESAMPLES)]
    percentiles = quantiles(boot, n=40, method="inclusive")
    lo, hi = percentiles[0] / divisor, percentiles[-1] / divisor
    wins = sum(sum(hits(cutoff, "tolmap") - hits(cutoff, "baseline")
                   for cutoff in repo["cutoffs"]) >= 0 for repo in active)
    gain = sum(differences) / divisor
    return {
        "baseline_mean": sum(b for _, b in scores) / divisor,
        "tolmap_mean": sum(a for a, _ in scores) / divisor,
        "mean_difference": gain, "ci95": [lo, hi], "repo_wins_or_ties": wins,
        "repos": len(active), "observations": len(scores),
        "pass": gain >= MIN_GAIN and lo > 0 and 2 * wins >= len(active),
        "zero_relevance_threshold_cutoffs": sum(
            cutoff[signal]["threshold"] == 0 for repo in active for cutoff in repo["cutoffs"]),
    }


def summary(result):
    lines = [
        "## Churn-prediction backtest (#375)", "",
        f"Actions run `{result['run_id']}` (attempt {result['run_attempt']}) "
        f"used tolmap and harness at `{result['head']}`.", "",
        "| Signal | Baseline P@10 | tolmap P@10 | Mean difference | Paired 95% CI | Repo wins or ties | Result |",
        "| --- | ---: | ---: | ---: | --- | ---: | --- |",
    ]
    for signal, row in result["signals"].items():
        lo, hi = row["ci95"]
        lines.append(f"| {signal} | {row['baseline_mean']:.4f} | {row['tolmap_mean']:.4f} | "
                     f"{row['mean_difference']:+.4f} | [{lo:+.4f}, {hi:+.4f}] | "
                     f"{row['repo_wins_or_ties']}/{row['repos']} | {'PASS' if row['pass'] else 'FAIL'} |")
    lines += ["", "| Repository | Source pin | First-parent commits | Cutoffs |",
              "| --- | --- | ---: | --- |"]
    for repo in result["repos"]:
        lines.append(f"| {repo['repo']} | `{repo['pin']}` | {repo['first_parent_commits']} | "
                     f"{repo.get('skip', len(repo['cutoffs']))} |")
    lines += ["", "Zero-count candidates share the highest rank of their tie. "
              "The truth threshold was zero at " + ", ".join(
                  f"{row['zero_relevance_threshold_cutoffs']}/{row['observations']} {signal} cutoffs"
                  for signal, row in result["signals"].items()) + ".",
              "", "The JSON artifact records each cutoff, historical window, prediction and hit."]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.out.mkdir(parents=True, exist_ok=True)
    fixture_path = Path(__file__).resolve().parents[1] / "data" / "fixtures.toml"
    specs = tomllib.loads(fixture_path.read_text())
    specs["semon"] = SEMON
    result = {
        "head": os.environ["GITHUB_SHA"], "run_id": os.environ["GITHUB_RUN_ID"],
        "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "settings": {"seed": SEED, "past_commits": PAST, "future_commits": FUTURE,
                     "cutoffs_per_repo": CUTOFFS, "precision_at": TOP, "resamples": RESAMPLES,
                     "min_gain": MIN_GAIN, "ci_lower_bound_gt": 0, "min_repo_win_share": 0.5,
                     "seam_weights": WEIGHTS,
                     "map_history": "tolmap native at T; raw counts use the 100 commits before T",
                     "candidate_universe": "tracked blob paths at T and their unordered pairs",
                     "missing_predictions": "misses in precision@10"},
        "repos": [],
    }
    for name, spec in sorted(specs.items()):
        with TemporaryDirectory(prefix=f"churn-{name}-") as directory:
            result["repos"].append(evaluate_repo(binary, name, spec, Path(directory)))
        (args.out / "partial.json").write_text(json.dumps(result, indent=2) + "\n")
    result["signals"] = {signal: aggregate(result["repos"], signal) for signal in ("files", "pairs")}
    (args.out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    (args.out / "summary.md").write_text(summary(result))
    (args.out / "partial.json").unlink()


if __name__ == "__main__":
    main()
