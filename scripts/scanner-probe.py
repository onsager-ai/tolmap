"""CI-only original/patched scanner counters, scaling and full AST equality.

Sources are read from pinned registry packages, never mutated there. The
instrumented copies and executables exist only in the runner's temp directory.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tarfile
import tomllib
import urllib.request

out = Path("scanner-evidence")
out.mkdir(exist_ok=True)
registry = Path.home() / ".cargo/registry/src"
core = next(registry.glob("*/tree-sitter-0.25.10"))
patched = Path("vendor/tree-sitter-python")
report = {"sha": os.environ.get("GITHUB_SHA"), "runs": [], "equal": [], "corpora": {},
          "tokens": ["NEWLINE", "INDENT", "DEDENT", "STRING_START", "STRING_CONTENT",
                     "ESCAPE_INTERPOLATION", "STRING_END", "COMMENT", "CLOSE_PAREN",
                     "CLOSE_BRACKET", "CLOSE_BRACE", "EXCEPT", "false"],
          "sampledExternalState10Mask": (1 << 3) | (1 << 7) | (1 << 11)}
instrument = r'''
static unsigned probe_mask;
static unsigned long long calls[4096], skips[4096], outcomes[4096][13];
static void probe_enter(const bool *symbols) {
    probe_mask = 0;
    for (unsigned i=0; i<12; i++) if (symbols[i]) probe_mask |= 1u<<i;
    calls[probe_mask]++;
}
static bool probe_return(bool value, TSLexer *lexer) {
    outcomes[probe_mask][value ? lexer->result_symbol : 12]++;
    return value;
}
void probe_report(void) {
    bool comma = false;
    for (unsigned i=0; i<4096; i++) if (calls[i]) {
        fprintf(stderr, "%s{\"mask\":%u,\"calls\":%llu,\"skips\":%llu,\"outcomes\":[",
                comma ? "," : "", i, calls[i], skips[i]);
        for (unsigned j=0; j<13; j++) fprintf(stderr, "%s%llu", j ? "," : "", outcomes[i][j]);
        fprintf(stderr, "]}"); comma = true;
    }
}
'''

with tempfile.TemporaryDirectory(prefix="scanner-probe-") as tmp:
    tmp = Path(tmp)
    archive = tmp / 'python.crate'
    urllib.request.urlretrieve('https://static.crates.io/crates/tree-sitter-python/tree-sitter-python-0.25.0.crate', archive)
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == '6bf85fd39652e740bf60f46f4cda9492c3a9ad75880575bf14960f775cb74a1c'
    with tarfile.open(archive) as tar:
        tar.extractall(tmp, filter='data')
    grammar = tmp / 'tree-sitter-python-0.25.0'
    binaries = {}
    for label, directory in [("original", grammar)] + ([("patched", patched)] if patched.exists() else []):
        source = (directory / "src/scanner.c").read_text()
        source = source.replace('static inline void advance(', instrument + '\nstatic inline void advance(', 1)
        source = source.replace('lexer->advance(lexer, true);', 'skips[probe_mask]++; lexer->advance(lexer, true);')
        begin = source.index('bool tree_sitter_python_external_scanner_scan(')
        end = source.index('unsigned tree_sitter_python_external_scanner_serialize(')
        scan = source[begin:end].replace('Scanner *scanner = (Scanner *)payload;',
               'Scanner *scanner = (Scanner *)payload; probe_enter(valid_symbols);', 1)
        scan = scan.replace('return true;', 'return probe_return(true, lexer);')
        scan = scan.replace('return false;', 'return probe_return(false, lexer);')
        scan = scan.replace('return has_content;', 'return probe_return(has_content, lexer);')
        scanner = tmp / f"{label}.c"
        scanner.write_text(source[:begin] + scan + source[end:])
        binary = tmp / label
        subprocess.run(["cc", "-O2", "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-D_DEFAULT_SOURCE",
            f"-I{core / 'include'}", f"-I{core / 'src'}", f"-I{directory / 'src'}",
            str(core / "src/lib.c"), str(directory / "src/parser.c"), str(scanner),
            "scripts/scanner-probe.c", "-o", str(binary)], check=True)
        binaries[label] = binary

    corpus_digest = hashlib.sha256()

    def run(name, source, compare=True, detail=True):
        path = tmp / "fixture.py"
        path.write_bytes(source)
        hashes = []
        for label, binary in binaries.items():
            ast = tmp / f"{label}.ast"
            with ast.open("wb") as output:
                try:
                    result = subprocess.run([str(binary), str(path)], stdout=output,
                                            stderr=subprocess.PIPE, timeout=30, check=True)
                    stats = json.loads(result.stderr)
                except subprocess.TimeoutExpired:
                    stats = {"timeout": True}
            digest = hashlib.sha256(ast.read_bytes()).hexdigest() if not stats.get("timeout") else None
            row = {"case": name, "build": label, "bytes": len(source),
                   "sourceSha256": hashlib.sha256(source).hexdigest(), "astSha256": digest, **stats}
            if detail:
                report["runs"].append(row)
            hashes.append(digest)
        if compare and len(hashes) == 2:
            assert all(hashes) and hashes[0] == hashes[1], name
            if detail:
                report["equal"].append(name)
            corpus_digest.update(name.encode() + b'\0' + source + hashes[0].encode())
        if detail:
            (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")

    for count in [1000, 2000, 4000, 8000, 16000]:
        run(f"comments-{count}", b'def measured():\n    return 7\n' +
            b'# performance fixture line 739\n' * count + b'# PERF_NEEDLE_739\n', compare=count <= 8000)

    if patched.exists():
        scaling = {row['case']: row for row in report['runs'] if row['build'] == 'patched'}
        work = lambda row: sum(state['skips'] for state in row['states'])
        assert work(scaling['comments-8000']) <= work(scaling['comments-2000']) * 4.1
        assert scaling['comments-16000']['seconds'] < 5
        # The full normal fixture is unchanged and the original is known to
        # exceed 30s. Equality uses scaled sources where both finish.
        original = binaries.pop("original")
        run("normal-full", b'def measured():\n    return 7\n' +
            b'# performance fixture line 739\n' * 30000 + b'# PERF_NEEDLE_739\n', compare=False)
        binaries["original"] = original
        run("long-full", b'LONG = "' + b'x' * 900000 + b'PERF_NEEDLE_739"\n')
        edge_cases = [
            'if x:\n    pass\n{comments}    pass\npass\n',
            'if x:\n    pass\n{comments}else:\n    pass\n',
            'if x:\n    pass\n{comments}elif y:\n    pass\n',
            'try:\n    pass\n{comments}except E:\n    pass\n',
            'try:\n    pass\n{comments}finally:\n    pass\n',
            'def f():\n    if x:\n        pass\n{comments}    return 1\n',
            'def f():\n    pass\n{comments}"string"\n',
            'x = (\n{comments}    1\n)\n',
            'x = 1 + \\\n{comments}2\n',
            'def broken(:\n{comments} return "unterminated\n',
            'x = f"{1 + (\n{comments}2)}"\n',
        ]
        edges = 0
        for template in edge_cases:
            for indent in ['', '    ', '        ', '\t', '\f    ']:
                # Do not format f-string braces in source templates.
                source = template.replace('{comments}', (indent + '# comment\n') * 4)
                for newline in ['\n', '\r\n']:
                    run(f"edge-{edges}", source.replace('\n', newline).encode(), detail=False)
                    edges += 1
        for source in [b'# EOF', b'if x:\n    pass\n# EOF', b'if x:\n    pass\n    # EOF']:
            run(f"eof-{edges}", source, detail=False)
            edges += 1
        report['corpora']['targeted'] = edges

        def clone(url, pin, name):
            dest = tmp / name
            subprocess.run(['git', 'init', '-q', str(dest)], check=True)
            subprocess.run(['git', '-C', str(dest), 'fetch', '-q', '--depth=1', url, pin], check=True)
            subprocess.run(['git', '-C', str(dest), 'checkout', '-q', 'FETCH_HEAD'], check=True)
            return dest

        fixtures = tomllib.loads(Path('data/fixtures.toml').read_text())
        for name, fixture in fixtures.items():
            if not isinstance(fixture, dict) or fixture.get('lang') != 'py':
                continue
            dest = clone(fixture['url'], fixture['commit'], name)
            paths = sorted((dest / fixture['pkg']).rglob('*.py'))
            for path in paths:
                run(f"{name}/{path.relative_to(dest)}", path.read_bytes(), detail=False)
            report['corpora'][name] = len(paths)

        upstream = clone('https://github.com/tree-sitter/tree-sitter-python.git',
                         '293fdc02038ee2bf0e2e206711b69c90ac0d413f', 'upstream')
        count = 0
        for path in sorted((upstream / 'test/corpus').rglob('*')):
            if not path.is_file():
                continue
            for match in re.finditer(r'(?ms)^={3,}\r?\n(.*?)\r?\n={3,}\r?\n(.*?)^-{3,}\r?$', path.read_text()):
                run(f"upstream/{path.name}/{count}", match[2].encode(), detail=False)
                count += 1
        assert count > 0, 'upstream corpus parser found no examples'
        report['corpora']['upstream'] = count
        report['differentialSha256'] = corpus_digest.hexdigest()
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")

print(json.dumps({"sha": report["sha"], "runs": len(report["runs"]), "equal": len(report["equal"])}))
