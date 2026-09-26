//! Which Go files are in the build for one fixed target (finding 54).
//!
//! The hand resolver links a Go import to the one file of the imported
//! package that declares each name the importer selects (finding 50). A name
//! that several files declare kept the whole package, and on prometheus
//! nearly all of those are build-tag variants: `model/labels` has a
//! `stringlabels`, a `slicelabels` and a `dedupelabels` implementation of
//! `Labels`, and `tsdb/fileutil` has a Unix and a Windows `OpenDir`. Only
//! one of each is compiled for any one build, so evaluating the files'
//! build constraints for one target breaks the tie.
//!
//! This follows `go/build` (`goodOSArchFile`, `parseFileHeader`,
//! `shouldBuild`) and `go/build/constraint`, with one difference: where Go
//! would guess, this answers "unknown", and the caller keeps finding 50's
//! answer. That is a `//go:build` line that does not parse, two of them, a
//! `// +build` literal Go would silently read as `ignore`, and any
//! `goexperiment.*` tag (the set a toolchain turns on by default changes
//! between releases). Evaluation is three-valued, so an unknown tag decides
//! nothing only where it could change the answer: `windows &&
//! goexperiment.x` is still false on Linux.
//!
//! Everything here reads the file's name and bytes and nothing else, so the
//! answer cannot depend on the order files are visited or on anything
//! outside the repository.

/// Whether a Go file is compiled for [`GO_BUILD_TARGET`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoBuild {
    In,
    Out,
    /// The file's constraint could not be evaluated with certainty.
    Unknown,
}

/// The one build the hand resolver evaluates Go build constraints for.
///
/// `linux/amd64` with the gc toolchain's default tags, cgo off and no custom
/// tags. Linux on amd64 is what the corpus's projects ship first and what CI
/// runs, and it is the platform scip-go, the oracle this resolver is scored
/// against, indexes on (finding 48's `hand-score` job). No custom tags
/// means a project's opt-in variants are out and its default variant is in:
/// prometheus's `labels_stringlabels.go` (`!slicelabels && !dedupelabels`)
/// is in, and `labels_slicelabels.go` (`slicelabels`) is out. cgo is off
/// because whether a C compiler is present is a property of the machine
/// that builds the map, not of the repository; the same commit has to give
/// the same map everywhere. (The oracle's runner has one, so a repository
/// with cgo-only files would differ from it there; prometheus has none in
/// the files that break a tie.) The release tags are Go 1.26's, the
/// toolchain CI installs for scip-go.
///
/// This bakes a platform into the map (owner decision, 2026-09-26, "Go build
/// tags", with that cost accepted): a Windows-only importer of `OpenDir` is
/// linked to `dir_unix.go` only if it is itself in this build, which it is
/// not, so it keeps finding 50's answer instead (`narrow_go_import`).
pub(crate) const GO_BUILD_TARGET: GoBuildTarget = GoBuildTarget {
    goos: "linux",
    goarch: "amd64",
    compiler: "gc",
    cgo: false,
    release_minor: 26,
    // `GOAMD64=v1`, the default, adds `amd64.v1`.
    tool_tags: &["amd64.v1"],
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct GoBuildTarget {
    goos: &'static str,
    goarch: &'static str,
    compiler: &'static str,
    cgo: bool,
    /// The release tags are `go1.1` through `go1.{release_minor}`.
    release_minor: u32,
    tool_tags: &'static [&'static str],
}

/// `go/build`'s `knownOS`: the operating systems a file name suffix can
/// name. `unix` is not one: since Go 1.19 it is valid only in a constraint.
const KNOWN_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];

/// `go/build`'s `knownArch`.
const KNOWN_ARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

/// `go/build`'s `unixOS`: what the `unix` constraint matches.
const UNIX_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "linux",
    "netbsd",
    "openbsd",
    "solaris",
];

/// Nesting beyond this is "unknown" rather than a deep recursion on a
/// hostile file. Real constraints nest two or three levels.
const MAX_DEPTH: usize = 64;

/// Whether `file` (a repository-relative path) is compiled for
/// [`GO_BUILD_TARGET`]. `imports` is the file's import paths, for cgo.
///
/// Each check can only exclude, so the first certain "no" decides; the
/// header's answer, which may be unknown, is consulted last.
pub(crate) fn go_build_status(file: &str, source: &[u8], imports: &[String]) -> GoBuild {
    GO_BUILD_TARGET.status(file, source, imports)
}

impl GoBuildTarget {
    fn status(&self, file: &str, source: &[u8], imports: &[String]) -> GoBuild {
        let name = file.rsplit('/').next().unwrap_or(file);
        // `go build` ignores files whose names start with `_` or `.`.
        if name.starts_with('_') || name.starts_with('.') || !self.matches_file_name(name) {
            return GoBuild::Out;
        }
        // A file that imports "C" is a cgo file, which `go build` leaves
        // out when cgo is off.
        if !self.cgo && imports.iter().any(|path| path == "C") {
            return GoBuild::Out;
        }
        match self.header(source) {
            Some(true) => GoBuild::In,
            Some(false) => GoBuild::Out,
            None => GoBuild::Unknown,
        }
    }

    /// `go/build`'s `matchTag`, three-valued: `None` for a tag whose value
    /// depends on the toolchain release rather than on the target.
    fn tag(&self, name: &str) -> Option<bool> {
        if name.starts_with("goexperiment.") || name == "boringcrypto" {
            return None;
        }
        if name == "cgo" {
            return Some(self.cgo);
        }
        if name == self.goos || name == self.goarch || name == self.compiler {
            return Some(true);
        }
        if name == "unix" {
            return Some(UNIX_OS.contains(&self.goos));
        }
        if self.tool_tags.contains(&name) {
            return Some(true);
        }
        if let Some(minor) = name.strip_prefix("go1.") {
            // Exactly `go1.N`: `go1.01` and `go1.21.0` are not release tags.
            if let Ok(value) = minor.parse::<u32>() {
                if value.to_string() == minor {
                    return Some((1..=self.release_minor).contains(&value));
                }
            }
        }
        Some(false)
    }

    /// `go/build`'s `goodOSArchFile`, plus `_test` files, which are never
    /// part of `go build` (the source walk already skips them).
    fn matches_file_name(&self, name: &str) -> bool {
        let stem = name.split('.').next().unwrap_or(name);
        if stem.ends_with("_test") {
            return false;
        }
        // Everything before the first `_` is ignored, so `linux.go` is not
        // constrained, but `x_linux.go` is.
        let Some(underscore) = stem.find('_') else {
            return true;
        };
        let parts = stem[underscore..].split('_').collect::<Vec<_>>();
        let n = parts.len();
        if n >= 2 && KNOWN_OS.contains(&parts[n - 2]) && KNOWN_ARCH.contains(&parts[n - 1]) {
            return self.tag(parts[n - 1]) == Some(true) && self.tag(parts[n - 2]) == Some(true);
        }
        if n >= 1 && (KNOWN_OS.contains(&parts[n - 1]) || KNOWN_ARCH.contains(&parts[n - 1])) {
            return self.tag(parts[n - 1]) == Some(true);
        }
        true
    }

    /// The file's build constraint, from `go/build`'s `parseFileHeader` and
    /// `shouldBuild`: a `//go:build` line anywhere in the leading comments
    /// decides alone; without one, every `// +build` line in the leading
    /// comments that a blank line separates from the package clause must
    /// hold. No constraint at all is `Some(true)`.
    fn header(&self, source: &[u8]) -> Option<bool> {
        let mut end = 0;
        let mut ended = false;
        let mut in_slash_star = false;
        let mut go_build = None;
        let mut rest = source;
        'lines: while !rest.is_empty() {
            let (raw, next) = match rest.iter().position(|&byte| byte == b'\n') {
                Some(at) => (&rest[..at], &rest[at + 1..]),
                None => (rest, &rest[rest.len()..]),
            };
            rest = next;
            let mut line = raw.trim_ascii();
            if line.is_empty() && !ended {
                end = source.len() - rest.len();
                continue;
            }
            if !line.starts_with(b"//") {
                ended = true;
            }
            if !in_slash_star && is_go_build_comment(line) {
                // Two `//go:build` lines make `go build` reject the file.
                // One after a block comment is misplaced: the rule is
                // "preceded only by blank lines and other line comments",
                // and rather than decide what `go build` makes of it, it is
                // unknown.
                if go_build.is_some() || ended {
                    return None;
                }
                go_build = Some(line);
            }
            while !line.is_empty() {
                if in_slash_star {
                    let Some(at) = line.windows(2).position(|pair| pair == b"*/") else {
                        continue 'lines;
                    };
                    in_slash_star = false;
                    line = line[at + 2..].trim_ascii();
                    continue;
                }
                if line.starts_with(b"//") {
                    continue 'lines;
                }
                if line.starts_with(b"/*") {
                    in_slash_star = true;
                    line = line[2..].trim_ascii();
                    continue;
                }
                // Non-comment text: the header ends here.
                break 'lines;
            }
        }
        if let Some(line) = go_build {
            let line = std::str::from_utf8(line).ok()?;
            return self.go_build_line(line);
        }
        let mut result = Some(true);
        for line in source[..end].split(|&byte| byte == b'\n') {
            let line = line.trim_ascii();
            let Some(fields) = plus_build_fields(line) else {
                continue;
            };
            let value = self.plus_build(std::str::from_utf8(fields).ok()?);
            result = and(result, value);
        }
        result
    }

    /// `constraint.Parse` on a `//go:build` line, then evaluation. A line
    /// that does not parse is `None`.
    fn go_build_line(&self, line: &str) -> Option<bool> {
        let expression = line.strip_prefix("//go:build")?;
        let mut parser = ExprParser {
            target: self,
            tokens: tokenize(expression)?,
            at: 0,
        };
        if parser.tokens.is_empty() {
            return None;
        }
        let value = parser.disjunction(0).ok()?;
        (parser.at == parser.tokens.len()).then_some(value)?
    }

    /// A `// +build` line's fields: space-separated fields are ORed, and a
    /// field's comma-separated literals are ANDed. Go reads a malformed
    /// literal (`!!x`, `a/b`) as the tag `ignore`, silently excluding the
    /// file; this answers "unknown" for it instead. A line with no fields is
    /// `ignore` in Go too, and that is a certain "no".
    fn plus_build(&self, fields: &str) -> Option<bool> {
        let mut any = Some(false);
        for field in fields.split_ascii_whitespace() {
            let mut all = Some(true);
            for literal in field.split(',') {
                let (negated, tag) = match literal.strip_prefix('!') {
                    Some(tag) => (true, tag),
                    None => (false, literal),
                };
                if !is_valid_tag(tag) {
                    return None;
                }
                let value = self.tag(tag).map(|value| value != negated);
                all = and(all, value);
            }
            any = or(any, all);
        }
        any
    }
}

/// `go/build`'s `isGoBuildComment`: `//go:build` followed by a space, a tab
/// or nothing.
fn is_go_build_comment(line: &[u8]) -> bool {
    match line.strip_prefix(b"//go:build") {
        Some(rest) => rest.is_empty() || rest[0] == b' ' || rest[0] == b'\t',
        None => false,
    }
}

/// The text after `+build` when `line` is a `// +build` comment
/// (`constraint.IsPlusBuild`), else `None`.
fn plus_build_fields(line: &[u8]) -> Option<&[u8]> {
    let rest = line.strip_prefix(b"//")?.trim_ascii();
    let fields = rest.strip_prefix(b"+build")?;
    (fields.is_empty() || fields[0] == b' ' || fields[0] == b'\t').then_some(fields)
}

/// `constraint`'s `isValidTag`: letters, digits, `_` and `.`, at least one.
fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token<'a> {
    Not,
    And,
    Or,
    Open,
    Close,
    Tag(&'a str),
}

/// `constraint`'s lexer. Anything else (a lone `&`, a `//` trailing
/// comment) is a syntax error, `None`.
fn tokenize(expression: &str) -> Option<Vec<Token<'_>>> {
    let mut tokens = Vec::new();
    let mut rest = expression;
    loop {
        rest = rest.trim_start_matches([' ', '\t']);
        let Some(first) = rest.chars().next() else {
            return Some(tokens);
        };
        let (token, length) = if rest.starts_with("&&") {
            (Token::And, 2)
        } else if rest.starts_with("||") {
            (Token::Or, 2)
        } else if first == '!' {
            (Token::Not, 1)
        } else if first == '(' {
            (Token::Open, 1)
        } else if first == ')' {
            (Token::Close, 1)
        } else {
            let length = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            if length == 0 {
                return None;
            }
            (Token::Tag(&rest[..length]), length)
        };
        tokens.push(token);
        rest = &rest[length..];
    }
}

/// `constraint`'s grammar: `||` binds loosest, then `&&`, then `!`; parens
/// group. Evaluated while parsing, which still reads every token, so a
/// syntax error anywhere is an error whatever the value so far.
struct ExprParser<'a, 't> {
    target: &'t GoBuildTarget,
    tokens: Vec<Token<'a>>,
    at: usize,
}

impl ExprParser<'_, '_> {
    fn peek(&self) -> Option<&Token<'_>> {
        self.tokens.get(self.at)
    }

    fn disjunction(&mut self, depth: usize) -> Result<Option<bool>, ()> {
        let mut value = self.conjunction(depth)?;
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            value = or(value, self.conjunction(depth)?);
        }
        Ok(value)
    }

    fn conjunction(&mut self, depth: usize) -> Result<Option<bool>, ()> {
        let mut value = self.unary(depth)?;
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            value = and(value, self.unary(depth)?);
        }
        Ok(value)
    }

    fn unary(&mut self, depth: usize) -> Result<Option<bool>, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        match self.tokens.get(self.at).cloned() {
            Some(Token::Not) => {
                self.at += 1;
                Ok(self.unary(depth + 1)?.map(|value| !value))
            }
            Some(Token::Open) => {
                self.at += 1;
                let value = self.disjunction(depth + 1)?;
                if self.peek() != Some(&Token::Close) {
                    return Err(());
                }
                self.at += 1;
                Ok(value)
            }
            Some(Token::Tag(tag)) => {
                self.at += 1;
                Ok(self.target.tag(tag))
            }
            _ => Err(()),
        }
    }
}

/// Kleene conjunction: false wins, then unknown.
fn and(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

/// Kleene disjunction: true wins, then unknown.
fn or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(file: &str, source: &str) -> GoBuild {
        go_build_status(file, source.as_bytes(), &[])
    }

    fn header(line: &str) -> GoBuild {
        status("x.go", &format!("{line}\n\npackage x\n"))
    }

    #[test]
    fn prometheus_label_variants_keep_only_the_default() {
        // The three `model/labels` implementations at the fixture's pin.
        assert_eq!(
            header("//go:build !slicelabels && !dedupelabels"),
            GoBuild::In
        );
        assert_eq!(header("//go:build slicelabels"), GoBuild::Out);
        assert_eq!(header("//go:build dedupelabels"), GoBuild::Out);
        // `tsdb/fileutil`'s `OpenDir`, after a licence comment.
        let unix = "// Copyright\n// licence\n\n//go:build !windows\n\npackage fileutil\n";
        assert_eq!(status("tsdb/fileutil/dir_unix.go", unix), GoBuild::In);
        let windows = unix.replace("!windows", "windows");
        assert_eq!(
            status("tsdb/fileutil/dir_windows.go", &windows),
            GoBuild::Out
        );
        // A tag and a file name must both hold.
        assert_eq!(
            status("mmap_amd64.go", "//go:build windows\n\npackage x\n"),
            GoBuild::Out
        );
    }

    #[test]
    fn go_build_expressions_follow_go_precedence() {
        for (line, expected) in [
            ("//go:build linux", GoBuild::In),
            ("//go:build linux && amd64 && gc && unix", GoBuild::In),
            (
                "//go:build darwin || dragonfly || freebsd || linux",
                GoBuild::In,
            ),
            ("//go:build !windows && !plan9 && !js", GoBuild::In),
            ("//go:build linux && forcedirectio", GoBuild::Out),
            ("//go:build linux && !forcedirectio", GoBuild::In),
            // `&&` binds tighter than `||`.
            ("//go:build windows && arm64 || linux", GoBuild::In),
            ("//go:build windows && (arm64 || linux)", GoBuild::Out),
            ("//go:build !(windows || darwin)", GoBuild::In),
            ("//go:build !!linux", GoBuild::In),
            ("//go:build\tlinux", GoBuild::In),
            ("//go:build cgo", GoBuild::Out),
            ("//go:build !cgo", GoBuild::In),
            ("//go:build ignore", GoBuild::Out),
            ("//go:build gccgo", GoBuild::Out),
            ("//go:build amd64.v1", GoBuild::In),
            ("//go:build go1.21", GoBuild::In),
            ("//go:build go1.26", GoBuild::In),
            ("//go:build go1.27", GoBuild::Out),
            ("//go:build !go1.18", GoBuild::Out),
            ("//go:build go1.021", GoBuild::Out),
            // A toolchain experiment is unknown, unless it cannot matter.
            ("//go:build goexperiment.jsonv2", GoBuild::Unknown),
            ("//go:build !goexperiment.jsonv2", GoBuild::Unknown),
            ("//go:build windows && goexperiment.jsonv2", GoBuild::Out),
            ("//go:build linux || goexperiment.jsonv2", GoBuild::In),
            // Syntax errors are unknown, never a guess.
            ("//go:build", GoBuild::Unknown),
            ("//go:build linux &", GoBuild::Unknown),
            ("//go:build (linux", GoBuild::Unknown),
            ("//go:build linux)", GoBuild::Unknown),
            ("//go:build linux amd64", GoBuild::Unknown),
            ("//go:build linux // trailing", GoBuild::Unknown),
            ("//go:build &&", GoBuild::Unknown),
            // Not a `//go:build` line at all.
            ("//go:buildlinux", GoBuild::In),
        ] {
            assert_eq!(header(line), expected, "{line}");
        }
        let deep = format!("//go:build {}linux{}", "(".repeat(200), ")".repeat(200));
        assert_eq!(header(&deep), GoBuild::Unknown);
    }

    #[test]
    fn plus_build_lines_are_ored_fields_of_anded_literals() {
        for (lines, expected) in [
            ("// +build linux", GoBuild::In),
            ("// +build windows darwin", GoBuild::Out),
            ("// +build windows linux", GoBuild::In),
            ("// +build linux,!cgo", GoBuild::In),
            ("// +build linux,cgo", GoBuild::Out),
            // Several lines must all hold.
            ("// +build linux\n// +build windows", GoBuild::Out),
            ("// +build linux darwin\n// +build amd64", GoBuild::In),
            // No fields is Go's `ignore`.
            ("// +build", GoBuild::Out),
            // A malformed literal Go would read as `ignore`.
            ("// +build !!linux", GoBuild::Unknown),
            ("// +build linux,a/b", GoBuild::Unknown),
            ("// +build goexperiment.x", GoBuild::Unknown),
            ("// +build windows,goexperiment.x", GoBuild::Out),
            // `//go:build` decides alone when both are present.
            ("//go:build linux\n// +build windows", GoBuild::In),
            ("// +build linux\n//go:build windows", GoBuild::Out),
            // Two `//go:build` lines: `go build` rejects the file.
            ("//go:build linux\n//go:build linux", GoBuild::Unknown),
        ] {
            assert_eq!(header(lines), expected, "{lines}");
        }
    }

    #[test]
    fn only_leading_comments_carry_a_constraint() {
        // A `+build` line not followed by a blank line is package doc.
        assert_eq!(
            status("x.go", "// +build windows\npackage x\n"),
            GoBuild::In
        );
        // A `//go:build` line in the doc comment still counts.
        assert_eq!(
            status("x.go", "//go:build windows\npackage x\n"),
            GoBuild::Out
        );
        // After the package clause nothing counts.
        assert_eq!(
            status(
                "x.go",
                "package x\n\n//go:build windows\n\n// +build windows\n"
            ),
            GoBuild::In
        );
        // A `//go:build` line after a block comment is misplaced, and one
        // inside a block comment is hidden.
        assert_eq!(
            status(
                "x.go",
                "/* licence\n */\n\n//go:build windows\n\npackage x\n"
            ),
            GoBuild::Unknown
        );
        assert_eq!(
            status("x.go", "/*\n//go:build windows\n*/\n\npackage x\n"),
            GoBuild::In
        );
        assert_eq!(status("x.go", ""), GoBuild::In);
    }

    #[test]
    fn file_names_constrain_os_and_arch() {
        for (file, expected) in [
            ("pkg/x.go", GoBuild::In),
            ("pkg/x_linux.go", GoBuild::In),
            ("pkg/x_amd64.go", GoBuild::In),
            ("pkg/x_linux_amd64.go", GoBuild::In),
            ("pkg/x_windows.go", GoBuild::Out),
            ("pkg/x_arm64.go", GoBuild::Out),
            ("pkg/x_linux_arm64.go", GoBuild::Out),
            ("pkg/x_windows_amd64.go", GoBuild::Out),
            // Only the last one or two elements count, and the first
            // element never does.
            ("pkg/windows_x.go", GoBuild::In),
            ("pkg/linux.go", GoBuild::In),
            ("pkg/windows.go", GoBuild::In),
            ("pkg/x_unix.go", GoBuild::In),
            ("pkg/x_windows_test.go", GoBuild::Out),
            ("pkg/x_test.go", GoBuild::Out),
            ("pkg/_x.go", GoBuild::Out),
            ("pkg/.x.go", GoBuild::Out),
        ] {
            assert_eq!(status(file, "package x\n"), expected, "{file}");
        }
    }

    #[test]
    fn a_cgo_file_is_out_with_cgo_off() {
        let imports = ["C".to_owned(), "fmt".to_owned()];
        assert_eq!(
            go_build_status("x.go", b"package x\n", &imports),
            GoBuild::Out
        );
        // Certain "no" beats an unknown header.
        assert_eq!(
            go_build_status("x.go", b"//go:build (\n\npackage x\n", &imports),
            GoBuild::Out
        );
        assert_eq!(
            go_build_status("x_windows.go", b"//go:build (\n\npackage x\n", &[]),
            GoBuild::Out
        );
    }
}
