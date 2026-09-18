"""Does warm-starting the partition generalise across repositories?"""
import json, subprocess, sys
import igraph as ig, leidenalg as la
import extract, pipeline, stability as S

REPOS = [("scrapy", "scrapy", 300), ("django", "django", 300),
         ("celery", "celery", 300), ("httpx", "httpx", 200)]
RES = 1.1


def prep(repo, pkg, tag):
    g = f"/tmp/claude-0/b_{tag}.json"
    extract.build(repo, pkg, g)
    return pipeline.prune(pipeline.blend(json.load(open(g))))


def partition_seeded(data, resolution, init=None):
    files = [n["f"] for n in data["nodes"]]
    idx = {f: i for i, f in enumerate(files)}
    g = ig.Graph(n=len(files)); g.vs["name"] = files
    g.add_edges([(idx[e["a"]], idx[e["b"]]) for e in data["edges"]])
    g.es["weight"] = [e["w"] for e in data["edges"]]
    kw = {}
    if init:
        nxt = max(init.values()) + 1; im = []
        for f in files:
            if f in init:
                im.append(init[f])
            else:
                im.append(nxt); nxt += 1
        kw["initial_membership"] = im
    p = la.find_partition(g, la.RBConfigurationVertexPartition, weights="weight",
                          resolution_parameter=resolution, n_iterations=-1,
                          seed=pipeline.SEED, **kw)
    return {files[i]: c for i, c in enumerate(p.membership)}, p


rows = []
for repo, pkg, back in REPOS:
    head = subprocess.run(["git", "-C", repo, "rev-parse", "HEAD"],
                          capture_output=True, text=True).stdout.strip()
    old = subprocess.run(["git", "-C", repo, "rev-parse", f"HEAD~{back}"],
                         capture_output=True, text=True).stdout.strip()
    if not old:
        print(f"{repo}: cannot go back {back} commits"); continue

    S.checkout(repo, old)
    dA = prep(repo, pkg, f"{repo}_old")
    mA, pA = partition_seeded(dA, RES); mA = pipeline.merge_tiny(mA, dA)

    S.checkout(repo, head)
    dB = prep(repo, pkg, f"{repo}_new")

    out = {}
    for label, init in (("cold", None), ("warm", mA)):
        mB, pB = partition_seeded(dB, RES, init)
        mB = pipeline.merge_tiny(mB, dB)
        m = S.match_districts(mB, mA)
        common = set(mB) & set(mA)
        kept = sum(1 for f in common
                   if mB[f] in m and m[mB[f]][0] == mA[f])
        out[label] = {"keep": 100 * kept / len(common) if common else 0,
                      "matched": len(m), "nd": len(set(mB.values())),
                      "q": pB.modularity}
    S.checkout(repo, head)

    c, w = out["cold"], out["warm"]
    rows.append((repo, len(dB["nodes"]), back, c, w))
    print(f"{repo:8s} n={len(dB['nodes']):4d}  back={back}  "
          f"keep {c['keep']:4.0f}% -> {w['keep']:4.0f}%   "
          f"matched {c['matched']}/{c['nd']} -> {w['matched']}/{w['nd']}   "
          f"Q {c['q']:.3f} -> {w['q']:.3f} ({100*(w['q']-c['q'])/c['q']:+.1f}%)")

json.dump([{"repo": r, "n": n, "back": b, "cold": c, "warm": w}
           for r, n, b, c, w in rows], open("batch_stability.json", "w"), indent=1)
