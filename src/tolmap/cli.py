"""tolmap — turn a source repository into a map.

    tolmap build ~/src/scrapy --pkg scrapy --lang py --name scrapy
    tolmap render scrapy django -o atlas.html
    tolmap stability ~/src/scrapy --pkg scrapy --back 300

`build` runs the whole pipeline and writes one self-contained JSON per repo;
`render` inlines any number of those into a standalone HTML map.
"""
import argparse
import json
import os
import sys
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
VIEWER = os.path.join(ROOT, "viewer", "template.html")

from . import extract, multi, pipeline, blobs, parcels, naming


def build(args):
    out_dir = args.out
    os.makedirs(out_dir, exist_ok=True)
    name = args.name or os.path.basename(os.path.abspath(args.repo))
    graph_path = os.path.join(out_dir, f"{name}.graph.json")
    layout_path = os.path.join(out_dir, f"{name}.layout.json")
    map_path = os.path.join(out_dir, f"{name}.json")

    print(f"[1/5] extract   {args.repo}/{args.pkg}  ({args.lang})")
    if args.lang == "py":
        extract.build(args.repo, args.pkg, graph_path)
    else:
        multi.build(args.repo, args.pkg, args.lang, graph_path)

    print(f"[2/5] partition resolution={args.resolution}")
    pipeline.run(graph_path, layout_path, args.resolution)

    print("[3/5] name     districts")
    layout = json.load(open(layout_path))
    by = defaultdict(list)
    for f, v in layout["nodes"].items():
        by[v["d"]].append(f)
    names = naming.name_districts(
        by, layout["nodes"],
        cache_path=os.path.join(out_dir, f"{name}.names.json"))
    blobs.NAMES[name] = {int(k): v for k, v in names.items()}
    for d, nm in sorted(names.items()):
        print(f"        d{d:<2} {len(by[d]):4d} files  {nm}")

    print("[4/5] geometry regions")
    blobs.build(name, layout_path, graph_path, map_path)

    if not args.no_parcels:
        print("[5/5] geometry weighted-voronoi plots")
        parcels.build(name, map_path)

    print(f"\nwrote {map_path}")
    return map_path


def render(args):
    data = {}
    for p in args.maps:
        path = p if os.path.exists(p) else os.path.join(args.out, f"{p}.json")
        if not os.path.exists(path):
            sys.exit(f"no such map: {p}")
        key = os.path.basename(path).replace(".json", "")
        data[key] = json.load(open(path))
    tpl = open(VIEWER).read()
    if "/*__DATA__*/" not in tpl:
        sys.exit("viewer template is missing its data placeholder")
    html = tpl.replace("/*__DATA__*/",
                       "const ALL = " + json.dumps(data, separators=(",", ":")) + ";")
    open(args.o, "w").write(html)
    mb = os.path.getsize(args.o) / 1e6
    print(f"wrote {args.o}  ({len(data)} map(s), {mb:.1f} MB)")


def stability(args):
    sys.path.insert(0, os.path.join(ROOT, "eval"))
    import stability as S                                  # noqa: E402
    S.repo_main(args.repo, args.pkg, args.back, args.resolution)


def main(argv=None):
    ap = argparse.ArgumentParser(prog="tolmap", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    b = sub.add_parser("build", help="index a repository into a map")
    b.add_argument("repo")
    b.add_argument("--pkg", default=".", help="source root inside the repo")
    b.add_argument("--lang", default="py", choices=["py", "go", "ts"])
    b.add_argument("--name", help="map name (default: repo directory name)")
    b.add_argument("--out", default="out")
    b.add_argument("--resolution", type=float, default=1.1,
                   help="Leiden resolution; higher gives more districts")
    b.add_argument("--no-parcels", action="store_true",
                   help="skip the weighted-Voronoi plot layer (faster)")
    b.set_defaults(fn=build)

    r = sub.add_parser("render", help="inline maps into a standalone HTML page")
    r.add_argument("maps", nargs="+")
    r.add_argument("--out", default="out")
    r.add_argument("-o", default="atlas.html")
    r.set_defaults(fn=render)

    s = sub.add_parser("stability", help="measure layout stability across commits")
    s.add_argument("repo")
    s.add_argument("--pkg", default=".")
    s.add_argument("--back", type=int, default=300)
    s.add_argument("--resolution", type=float, default=1.1)
    s.set_defaults(fn=stability)

    args = ap.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
