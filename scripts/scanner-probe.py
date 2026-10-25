"""CI-only original/patched scanner counters, scaling and full AST equality.

Sources are read from pinned registry packages, never mutated there. The
instrumented copies and executables exist only in the runner's temp directory.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

out = Path("scanner-evidence")
out.mkdir(exist_ok=True)
registry = Path.home() / ".cargo/registry/src"
grammar = next(registry.glob("*/tree-sitter-python-0.25.0"))
core = next(registry.glob("*/tree-sitter-0.25.10"))
patched = Path("vendor/tree-sitter-python")
report = {"sha": os.environ.get("GITHUB_SHA"), "runs": [], "equal": []}
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

    def run(name, source, compare=True):
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
            report["runs"].append(row)
            hashes.append(digest)
        if compare and len(hashes) == 2:
            assert all(hashes) and hashes[0] == hashes[1], name
            report["equal"].append(name)
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")

    for count in [1000, 2000, 4000, 8000, 16000]:
        run(f"comments-{count}", b'def measured():\n    return 7\n' +
            b'# performance fixture line 739\n' * count + b'# PERF_NEEDLE_739\n', compare=count <= 8000)

print(json.dumps({"sha": report["sha"], "runs": len(report["runs"]), "equal": len(report["equal"])}))
