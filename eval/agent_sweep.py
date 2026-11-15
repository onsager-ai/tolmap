#!/usr/bin/env python3
"""CI-only size sweep for #303. Source pins deliberately are not parity fixtures."""

PINS = {
    "hono": {"repo": "honojs/hono", "sha": "f6fbaf0299c48eea9e656e03e2ffe7ac755e8823", "diff": "0809e48947c43ae20542bc14c0f750b6c6585b4d", "band": "small", "target": "src/utils/cookie.ts", "entry": "src/index.ts"},
    "fastapi": {"repo": "fastapi/fastapi", "sha": "a3260d49e97b0072c4c0c161d9f63cc3826584b5", "diff": "a3260d49e97b0072c4c0c161d9f63cc3826584b5", "band": "small-mid", "target": "fastapi/params.py", "entry": "fastapi/applications.py"},
    "uv": {"repo": "astral-sh/uv", "sha": "d5c3f174417418c24ec3c276e01b0f62b961b13c", "diff": "d5c3f174417418c24ec3c276e01b0f62b961b13c", "band": "mid", "target": "crates/uv-small-str/src/lib.rs", "entry": "crates/uv/src/main.rs"},
    "transformers": {"repo": "huggingface/transformers", "sha": "3a134bc37f7c22c801b618e289b0255425f22647", "diff": "3a134bc37f7c22c801b618e289b0255425f22647", "band": "large", "target": "src/transformers/modeling_outputs.py", "entry": "src/transformers/__init__.py"},
    "vscode": {"repo": "microsoft/vscode", "sha": "3ea3ca1d231cae6826355b86b03635ebf0d93068", "diff": "3ea3ca1d231cae6826355b86b03635ebf0d93068", "band": "large", "target": "src/vs/base/common/lifecycle.ts", "entry": "src/main.ts"},
}

import argparse
import ast
from collections import Counter
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import time


def encoded(value):
    return (json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


def source_imports(path, text, files):
    """Independent source audit, not map E. Return resolved local file paths.

    Python: AST Import/ImportFrom including TYPE_CHECKING and function imports;
    module imports resolve to module files, from-imports also resolve submodules.
    TS: static import/export-from + literal dynamic import/require, relative paths
    and VS Code's vs/ alias. Rust: explicit uv_small_str crate imports only (the
    chosen target has no local outbound use). No symbol/re-export chasing, macros,
    computed imports, Python __getattr__, TS paths aliases or implicit preludes.
    """
    found = set()
    if path.endswith('.py'):
        module_files = {}
        for name in files:
            if not name.endswith('.py'):
                continue
            module = name.removeprefix('src/').removesuffix('.py').replace('/', '.')
            module_files[module.removesuffix('.__init__')] = name
        package = path.removeprefix('src/').removesuffix('.py').replace('/', '.').split('.')[:-1]
        for node in ast.walk(ast.parse(text)):
            modules = []
            if isinstance(node, ast.Import):
                modules = [alias.name for alias in node.names]
            elif isinstance(node, ast.ImportFrom):
                prefix = package[:len(package) - node.level + 1] if node.level else []
                module = '.'.join(prefix + ([node.module] if node.module else []))
                modules = [module] + [f'{module}.{alias.name}' for alias in node.names]
            for module in modules:
                if module in module_files:
                    found.add(module_files[module])
    elif path.endswith(('.ts', '.tsx')):
        specs = re.findall(r'''(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)['"]([^'"\n]+)['"]''', text)
        for spec in specs:
            if spec.startswith('.'):
                stem = os.path.normpath(str(PurePosixPath(path).parent / spec))
            elif spec.startswith('vs/'):
                stem = 'src/' + spec
            else:
                continue
            if stem.endswith('.js'):
                stem = stem[:-3]
            for candidate in (stem, stem + '.ts', stem + '.tsx', stem + '/index.ts'):
                if candidate in files:
                    found.add(candidate)
                    break
    elif path.endswith('.rs'):
        # uv-small-str is a single-file workspace crate. Include use blocks and
        # fully qualified name uses; exclude its own manifest/mod declarations.
        if re.search(r'\buv_small_str\s*::', text):
            found.add('crates/uv-small-str/src/lib.rs')
    found.discard(path)
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', choices=PINS, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--mem-bytes', default='15032385536')
    args = parser.parse_args()
    pin = PINS[args.repo]
    binary, out = args.binary.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    checkout = out / 'checkout'
    commands = []
    summary = {'repo': args.repo, 'pin': pin, 'state': 'running', 'tasks': []}

    def save(label, value):
        data = encoded(value)
        (out / f'{label}.json').write_bytes(data)
        return len(data)

    def run(label, argv, cwd=None, allow_empty=False):
        argv = [str(a) for a in argv]
        start = time.monotonic()
        # Each expensive command is bounded independently; a failed job retains
        # its partial measurements and logs, rather than omitting the repository.
        result = subprocess.run(argv, cwd=cwd, capture_output=True)
        commands.append({'label': label, 'argv': argv, 'cwd': str(cwd),
                         'exit_code': result.returncode, 'wall_seconds': time.monotonic() - start,
                         'stdout_bytes': len(result.stdout)})
        (out / f'{label}.stdout').write_bytes(result.stdout)
        (out / f'{label}.stderr').write_bytes(result.stderr)
        save('commands', commands)
        if result.returncode != 0 and not (allow_empty and result.returncode == 1):
            result.check_returncode()
        return result.stdout

    def query(label, *argv):
        return run(label, ['timeout', '30m', 'prlimit', f'--as={args.mem_bytes}', '--', binary, *argv], checkout)

    def projection(label, value):
        return save(label, value)

    def truncation(payload):
        doc = json.loads(payload)
        return {key: doc[key] for key in ('total', 'limit', 'truncated', 'omitted', 'next_limit')}

    def cost(byte_count, calls, **extra):
        return {'bytes': byte_count, 'calls': calls, **extra}

    try:
        run('clone', ['git', 'clone', '--filter=blob:none', '--no-checkout', '--depth', '4000',
                      f'https://github.com/{pin["repo"]}.git', checkout])
        run('fetch-pin', ['git', 'fetch', '--depth', '4000', 'origin', pin['sha']], checkout)
        run('checkout', ['git', 'checkout', '--detach', pin['sha']], checkout)
        actual = run('source-sha', ['git', 'rev-parse', 'HEAD'], checkout).decode().strip()
        if actual != pin['sha']:
            raise ValueError(f'checkout mismatch: {actual}')
        run('diff-first-parent', ['git', 'rev-parse', pin['diff'] + '^1'], checkout)
        query('detect', 'detect', checkout)
        map_dir = out / 'map'
        query('build', 'build', checkout, '--all-sources', '--name', args.repo, '--out', map_dir)
        map_path = map_dir / f'{args.repo}.json'
        graph = json.loads(map_path.read_bytes())
        files, nodes, edges, names = graph['F'], graph['N'], graph['E'], graph['names']
        file_set = set(files)
        summary.update(file_count=len(files), build_wall_seconds=commands[-1]['wall_seconds'],
                       map_sha256=hashlib.sha256(map_path.read_bytes()).hexdigest(),
                       raw_bytes={'map': map_path.stat().st_size,
                                  'symbols': (map_dir / f'{args.repo}.symbols.json').stat().st_size})
        save('measurements', summary)
        target = pin['target']
        if target not in file_set:
            raise ValueError(f'audit target outside tolmap scope: {target}')

        # Retain rg evidence and complete target source for a human audit. Ground
        # truth is independently derived from source, restricted to map F so the
        # denominator is explicit; it never uses tolmap import edges.
        patterns = {'hono': r'utils/cookie|[./]cookie[\x27\"]',
                    'fastapi': r'fastapi\.params|from\s+(fastapi|\.)\s+import|from\s+\.params',
                    'uv': r'uv_small_str\s*::',
                    'transformers': r'modeling_outputs',
                    'vscode': r'(common|\.)/lifecycle'}
        rg = run('ground-truth-rg', ['rg', '-n', '-U', patterns[args.repo], '.',
                 '--glob', '*.py', '--glob', '*.ts', '--glob', '*.tsx', '--glob', '*.rs'],
                 checkout, allow_empty=True)
        target_bytes = (checkout / target).read_bytes()
        (out / 'ground-truth-target.stdout').write_bytes(target_bytes)
        imports = source_imports(target, target_bytes.decode(), file_set)
        importers = set()
        for path in files:
            if Path(path).suffix != Path(target).suffix and not (path.endswith('.tsx') and target.endswith('.ts')):
                continue
            text = (checkout / path).read_text(encoding='utf-8')
            if target in source_imports(path, text, file_set):
                importers.add(path)
        truth = {'in': sorted(importers), 'out': sorted(imports)}
        save('ground-truth', {'target': target, 'expected': truth,
                             'scope': 'explicit static file imports within map F, including tests and type-only imports; no re-export chasing or computed imports',
                             'audit_parser': source_imports.__doc__})
        if not importers:
            raise ValueError('chosen target has no independently audited fan-in')

        # A deterministic grep/read orientation: top-level listing, root README
        # and manifests, then the first 120 lines of one pinned entry file.
        listing = run('grep-orient-list', ['git', 'ls-tree', '--name-only', 'HEAD'], checkout)
        manifests = b''
        for name in ('README.md', 'Cargo.toml', 'package.json', 'pyproject.toml'):
            if (checkout / name).is_file():
                manifests += f'--- {name}\n'.encode() + (checkout / name).read_bytes()
        entry_source = b''.join((checkout / pin['entry']).read_bytes().splitlines(keepends=True)[:120])
        (out / 'grep-orient-read.stdout').write_bytes(manifests + entry_source)
        # The query and JSON orientation recipes inspect the same manifests for
        # declared entries; L landmarks are also returned by the projection.
        (out / 'entry-manifests.stdout').write_bytes(manifests)
        overview = query('overview', 'overview', '--map', map_path)
        where = query('where', 'where', pin['entry'], '--map', map_path)
        deps = query('deps', 'deps', target, '--direction', 'both', '--map', map_path)
        complete = query('deps-complete', 'deps', target, '--direction', 'both', '--limit', str(len(files) * 2), '--map', map_path)
        check = query('check', 'check', checkout, '--base', pin['diff'] + '^1', '--head', pin['diff'],
                      '--all-sources', '--report-only', '--format', 'json')
        debt = query('debt-full', 'debt', '--map', map_path, '--format', 'json')
        summary['raw_bytes']['debt'] = len(debt)
        hot = query('hotspots', 'debt', '--map', map_path, '--kind', 'hotspot', '--top', '5', '--format', 'json')
        cycles = query('cycles', 'debt', '--map', map_path, '--kind', 'cycle', '--top', '5', '--format', 'json')

        district = lambda i: names[str(nodes[i][0])]
        counts = Counter(n[0] for n in nodes)
        orient_bytes = projection('projection-orient', {
            'districts': [{'name': names[str(key)], 'files': counts[key]} for key in sorted(counts)],
            'entries': [files[row[0]] for row in graph['L'] if row[1] == 'entry'],
            'entry': {'path': pin['entry'], 'district': district(files.index(pin['entry'])) if pin['entry'] in file_set else None}})
        index = files.index(target)
        dep_rows = [{'path': files[b if a == index else a], 'direction': 'out' if a == index else 'in',
                     'district': district(b if a == index else a)} for a, b in edges if a == index or b == index]
        dep_bytes = projection('projection-deps', dep_rows)
        diff = run('diff-paths', ['git', 'diff', '--name-only', pin['diff'] + '^1', pin['diff']], checkout)
        patch = run('grep-review-patch', ['git', 'show', '--format=fuller', '--no-ext-diff', pin['diff']], checkout)
        base_tree, base_dir = out / 'base-checkout', out / 'base-map'
        run('base-worktree', ['git', 'worktree', 'add', '--detach', base_tree, pin['diff'] + '^1'], checkout)
        query('base-build', 'build', base_tree, '--all-sources', '--name', 'base', '--out', base_dir)
        base_path = base_dir / 'base.json'
        base = json.loads(base_path.read_bytes())
        base_files = {path: i for i, path in enumerate(base['F'])}
        impact_bytes = projection('projection-impact', [
            {'path': path, 'district': base['names'][str(base['N'][base_files[path]][0])] if path in base_files else None}
            for path in diff.decode().splitlines()])
        debt_doc = json.loads(debt)
        debt_bytes = projection('projection-debt', {'hotspots': debt_doc['hotspots'][:5], 'cycles': debt_doc['cycles'][:5]})
        # Grep debt is an explicitly weaker triage heuristic: read paths/import
        # statements with line numbers, then five largest scoped source files.
        # It cannot produce map hotspots (history x complexity) or district cycles.
        grep_debt = run('grep-debt-imports', ['rg', '-n', r'^\s*(import |from |use |pub use |export .*from)',
                        '.', '--glob', '*.py', '--glob', '*.ts', '--glob', '*.tsx', '--glob', '*.rs'], checkout, allow_empty=True)
        largest = sorted(files, key=lambda p: (-(checkout / p).stat().st_size, p))[:5]
        debt_read = b''.join(f'--- {p}\n'.encode() + (checkout / p).read_bytes() for p in largest)
        (out / 'grep-debt-read.stdout').write_bytes(debt_read)
        map_bytes = summary['raw_bytes']['map']
        symbols_bytes = summary['raw_bytes']['symbols']
        summary['tasks'] = [
            {'task': 'orient', 'query': cost(len(manifests) + len(overview) + len(where), 3, truncation={'overview': truncation(overview), 'where': truncation(where)}),
             'full_json': cost(map_bytes + symbols_bytes + len(manifests) + orient_bytes, 4),
             'filtered_only': cost(len(manifests) + orient_bytes, 2),
             'grep_read': cost(len(listing) + len(manifests) + len(entry_source), 2, limit='entry read: first 120 lines; no semantic districts')},
            {'task': 'before-edit', 'query': cost(len(deps), 1, truncation=truncation(deps)),
             'full_json': cost(map_bytes + dep_bytes, 2), 'filtered_only': cost(dep_bytes, 1),
             'grep_read': cost(len(rg) + len(target_bytes), 2, limit='literal rg candidates and full target source; manual resolution required')},
            {'task': 'review-impact', 'query': cost(len(check), 1),
             'full_json': cost(base_path.stat().st_size + len(diff) + impact_bytes, 3),
             'filtered_only': cost(len(diff) + impact_bytes, 2),
             'grep_read': cost(len(diff) + len(patch), 2, limit='changed paths and patch only; no districts, changed edges or modularity')},
            {'task': 'debt-pick', 'query': cost(len(hot) + len(cycles), 2,
                 truncation={kind: {'total': json.loads(payload)['unfiltered_counts'][{'hotspots': 'hotspot', 'cycles': 'cycle'}[kind]],
                                    'returned': len(json.loads(payload)[kind]),
                                    'omitted': json.loads(payload)['unfiltered_counts'][{'hotspots': 'hotspot', 'cycles': 'cycle'}[kind]] - len(json.loads(payload)[kind])}
                             for kind, payload in [('hotspots', hot), ('cycles', cycles)]}),
             'full_json': cost(len(debt) + debt_bytes, 2), 'filtered_only': cost(debt_bytes, 1),
             'grep_read': cost(len(grep_debt) + len(debt_read), 2, limit='import grep + five largest files, not equivalent hotspots or cycles')},
        ]

        def recall(rows):
            result = {}
            for direction in ('in', 'out'):
                expected = set(truth[direction])
                actual = {row['path'] for row in rows if row['direction'] == direction}
                result[direction] = {'expected': len(expected), 'matched': len(actual & expected),
                                     'recall': len(actual & expected) / len(expected) if expected else None,
                                     'misses': sorted(expected - actual), 'extra': sorted(actual - expected)}
            return result
        summary.update(state='complete', recall_default=recall(json.loads(deps)['results']),
                       recall_complete=recall(json.loads(complete)['results']), recall_projection=recall(dep_rows),
                       complete_query_bytes=len(complete),
                       recall_grep_read='source candidate evidence retained; manual audit required, not assumed 100%',
                       calls_scope='independent recipes; separate stdout/raw reads/projections each count; build, audit and provenance excluded',
                       history='depth 4000 at pin; fixed horizon, not full repository history',
                       bytes_scope='stdout/file bytes delivered, not tokens or internal IO; each recipe once; no agent success trial')
    except Exception as error:
        summary.update(state='failed', error=str(error))
        raise
    finally:
        save('measurements', summary)


if __name__ == '__main__':
    main()
