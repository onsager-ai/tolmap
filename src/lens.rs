//! Lenses (issue #241): a named, versioned overlay of areas, paths and notes
//! over an existing map. A lens never changes the map: `resolve` is a pure
//! function of `(Lens, MapDocument)` and takes the map by shared reference.
//!
//! The lens file is TOML, kept in the analysed repository at
//! `.tolmap/lenses/<name>.toml`; docs/API.md describes the service resource
//! that reads it at a pinned commit. The types below define the file format
//! and the resolved report once, in Rust; the TypeScript bindings are
//! generated from them.
//!
//! Plain `//` comments on exported types, not doc comments: ts-rs copies doc
//! comments into the bindings.
//!
//! Everything reported is a lower bound. A computed hop is a route through the
//! map's file-level `E` edges (imports, plus SCIP references where the index
//! used them). The absence of a route means "none found in the edges the map
//! has", never "none exists". Symbol stops are resolved to their file for
//! routing; no symbol-level edge is consulted yet.
use crate::schema::MapDocument;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use ts_rs::TS;

pub const LENS_VERSION: u32 = 1;
pub const REPORT_VERSION: u32 = 1;

// The fixed palette. A lens names a token, never a hex value, so the viewer
// owns contrast in both themes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LensColor {
    Blue,
    Teal,
    Green,
    Amber,
    Orange,
    Red,
    Purple,
    Grey,
}

// How the hop ARRIVING at a stop is established. `computed` (the default) is
// resolved from map edges; `declared` is a hand-written link, with a reason,
// for a connection static analysis cannot see (a fetch to a handler, a queue,
// IPC, a database).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HopMode {
    #[default]
    Computed,
    Declared,
}

// Members and stops are strings:
//   `src/a.ts`            a file
//   `src/**/*.ts`         a glob (`*` and `?` stay inside one path segment,
//                         `**` spans segments); only in area members
//   `src/a.ts#name`       a symbol of that file, by the name in the map's `S`
//   `src/a.ts#name@12`    the same, disambiguated by its start line
// A note anchor is one of the file or symbol forms, or `district:<name>`.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LensArea {
    pub label: String,
    pub color: LensColor,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LensStop {
    pub at: String,
    // The hop from the previous stop to this one. Not allowed on the first.
    #[serde(default)]
    pub hop: HopMode,
    // Required when `hop` is `declared`.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LensPath {
    pub name: String,
    pub stops: Vec<LensStop>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LensNote {
    pub anchor: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct Lens {
    pub version: u32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub areas: Vec<LensArea>,
    #[serde(default)]
    pub paths: Vec<LensPath>,
    #[serde(default)]
    pub notes: Vec<LensNote>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LensError {
    // The text is not TOML of this shape.
    Parse(String),
    // Well-formed, but breaks a rule the resolver relies on.
    Invalid(String),
}

impl std::fmt::Display for LensError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LensError::Parse(m) => write!(f, "lens parse error: {m}"),
            LensError::Invalid(m) => write!(f, "invalid lens: {m}"),
        }
    }
}

impl std::error::Error for LensError {}

impl Lens {
    pub fn parse(text: &str) -> Result<Lens, LensError> {
        let lens: Lens = toml::from_str(text).map_err(|e| LensError::Parse(e.to_string()))?;
        lens.validate()?;
        Ok(lens)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a Lens always serialises")
    }

    fn validate(&self) -> Result<(), LensError> {
        let bad = |m: String| Err(LensError::Invalid(m));
        if self.version != LENS_VERSION {
            return bad(format!(
                "unsupported lens version {}, expected {LENS_VERSION}",
                self.version
            ));
        }
        let mut labels = std::collections::BTreeSet::new();
        for area in &self.areas {
            if area.label.trim().is_empty() || !labels.insert(&area.label) {
                return bad(format!("area label {:?} is empty or repeated", area.label));
            }
            if area.members.is_empty() {
                return bad(format!("area {:?} has no members", area.label));
            }
            for m in &area.members {
                check_spec(m, true).or_else(|e| bad(format!("area {:?}: {e}", area.label)))?;
            }
        }
        let mut names = std::collections::BTreeSet::new();
        for path in &self.paths {
            if !valid_name(&path.name) || !names.insert(&path.name) {
                return bad(format!(
                    "path name {:?} is not [a-z0-9_-]{{1,64}} or is repeated",
                    path.name
                ));
            }
            if path.stops.len() < 2 {
                return bad(format!("path {:?} needs at least two stops", path.name));
            }
            for (i, stop) in path.stops.iter().enumerate() {
                check_spec(&stop.at, false)
                    .or_else(|e| bad(format!("path {:?} stop {i}: {e}", path.name)))?;
                if i == 0 && (stop.hop == HopMode::Declared || stop.reason.is_some()) {
                    return bad(format!(
                        "path {:?}: the first stop has no incoming hop",
                        path.name
                    ));
                }
                if stop.hop == HopMode::Declared
                    && stop.reason.as_deref().is_none_or(|r| r.trim().is_empty())
                {
                    return bad(format!(
                        "path {:?} stop {i}: a declared hop requires a reason",
                        path.name
                    ));
                }
            }
        }
        for (i, note) in self.notes.iter().enumerate() {
            if note.text.trim().is_empty() {
                return bad(format!("note {i} has no text"));
            }
            match note.anchor.strip_prefix("district:") {
                Some(name) if !name.is_empty() => {}
                Some(_) => return bad(format!("note {i}: empty district name")),
                None => check_spec(&note.anchor, false)
                    .or_else(|e| bad(format!("note {i} anchor: {e}")))?,
            }
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

// Lens names (the file stem in `.tolmap/lenses/`) use the same charset as
// path names, and must start with a letter or digit.
pub fn valid_lens_name(name: &str) -> bool {
    valid_name(name) && !name.starts_with(['-', '_'])
}

#[derive(Debug, PartialEq, Eq)]
enum Spec<'a> {
    Glob(&'a str),
    File(&'a str),
    Symbol {
        file: &'a str,
        name: &'a str,
        line: Option<usize>,
    },
}

fn parse_spec(spec: &str) -> Spec<'_> {
    if let Some((file, rest)) = spec.split_once('#') {
        let (name, line) = match rest.rsplit_once('@') {
            Some((name, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
                (name, line.parse().ok())
            }
            _ => (rest, None),
        };
        return Spec::Symbol { file, name, line };
    }
    if spec.contains(['*', '?']) {
        Spec::Glob(spec)
    } else {
        Spec::File(spec)
    }
}

fn check_spec(spec: &str, allow_glob: bool) -> Result<(), String> {
    let file = match parse_spec(spec) {
        Spec::Glob(g) if allow_glob => g,
        Spec::Glob(_) => {
            return Err(format!(
                "{spec:?}: a glob is only allowed as an area member"
            ))
        }
        Spec::File(f) => f,
        Spec::Symbol { file, name, .. } => {
            if name.is_empty() {
                return Err(format!("{spec:?}: empty symbol name"));
            }
            if file.contains(['*', '?']) {
                return Err(format!("{spec:?}: a symbol ID names one file, not a glob"));
            }
            file
        }
    };
    // The same relative-path rule the source reader applies, so a spec can
    // never name anything outside the repository tree.
    if crate::source::valid_path(file) {
        Ok(())
    } else {
        Err(format!("{spec:?}: not a relative repository path"))
    }
}

// `*` and `?` match within one segment; a `**` segment matches zero or more
// whole segments. Brackets and braces are literal: `[id].tsx` is a real name.
fn glob_matches(pattern: &str, path: &str) -> bool {
    fn segment(p: &[u8], s: &[u8]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((b'*', rest)) => (0..=s.len()).any(|k| segment(rest, &s[k..])),
            Some((b'?', rest)) => !s.is_empty() && segment(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && segment(rest, &s[1..]),
        }
    }
    fn segments(p: &[&str], s: &[&str]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((&"**", rest)) => (0..=s.len()).any(|k| segments(rest, &s[k..])),
            Some((head, rest)) => {
                !s.is_empty()
                    && segment(head.as_bytes(), s[0].as_bytes())
                    && segments(rest, &s[1..])
            }
        }
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments(&p, &s)
}

// ---- resolved output -------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct SymbolTarget {
    pub name: String,
    // Index into the map's symbol kinds (`SymbolRow` column 1).
    pub kind: usize,
    pub start: usize,
    pub end: usize,
}

// A member, stop or anchor found in the map: the file (an index into `F`),
// its path, its district id, and the symbol when the spec named one.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct LensTarget {
    pub file: usize,
    pub path: String,
    pub district: usize,
    pub symbol: Option<SymbolTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct ResolvedMember {
    // The spec that produced it; a glob yields one member per matching file.
    pub spec: String,
    pub target: LensTarget,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct ResolvedArea {
    pub label: String,
    pub color: LensColor,
    // Sorted by path, then symbol; one entry per distinct target.
    pub members: Vec<ResolvedMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct ResolvedStop {
    pub spec: String,
    // None when the stop did not resolve; the reason is in `unresolved`.
    pub target: Option<LensTarget>,
}

// How a computed hop was found. Mirrors the viewer's path mode
// (web/src/map/graph.ts `findRoute`): a directed route from the first stop's
// file, else one from the second's reported reversed, else an undirected one.
// `same-file` is containment, not an edge: both stops sit in one file.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum RouteKind {
    #[serde(rename = "imports")]
    Imports,
    #[serde(rename = "imported-by")]
    ImportedBy,
    #[serde(rename = "undirected")]
    Undirected,
    #[serde(rename = "same-file")]
    SameFile,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HopSource {
    Computed,
    Declared,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct ResolvedHop {
    // Indices into the path's `stops`; always `to == from + 1`.
    pub from: usize,
    pub to: usize,
    pub source: HopSource,
    // Set for a computed hop that found a route.
    pub edge_kind: Option<RouteKind>,
    // The files the route passes through, endpoints included, in order from
    // `from`'s file to `to`'s. Empty unless computed and found.
    pub via: Vec<String>,
    // The declared hop's reason; null for a computed hop.
    pub reason: Option<String>,
    // False when an endpoint did not resolve or no route was found.
    pub resolved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct ResolvedPath {
    pub name: String,
    pub stops: Vec<ResolvedStop>,
    pub hops: Vec<ResolvedHop>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct ResolvedNote {
    pub anchor: String,
    pub text: String,
    // Set for a `district:<name>` anchor.
    pub district: Option<usize>,
    // Set for a file or symbol anchor.
    pub target: Option<LensTarget>,
}

// Distinct kinds, so a consumer can treat a stale lens differently from a
// broken one.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedKind {
    // A file path (or a stop or anchor) that is not in the map.
    MissingPath,
    // Git found a rename; the target remains unresolved until the lens is edited.
    RenamedFile,
    // A glob that matches no file of the map.
    EmptyGlob,
    // The file exists but has no symbol of that name (or start line).
    UnknownSymbol,
    // A symbol name or district name that matches more than one.
    Ambiguous,
    // `district:<name>` that is not a district name of the map.
    UnknownDistrict,
    // A computed hop with no route in the map's edges. A lower bound: the
    // files may still be connected by something extraction cannot see.
    NoRoute,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct Unresolved {
    pub kind: UnresolvedKind,
    // Where in the lens, e.g. `paths.sync.stops[1]`.
    pub location: String,
    pub spec: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub renamed: Option<FileRename>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct FileRename {
    pub from: String,
    pub to: String,
    pub commit: String,
}

// Enrich only absent literal paths. A rename is evidence, never a replacement
// target; globs and existing paths keep their ordinary resolution semantics.
pub fn report_renames(report: &mut LensReport, renames: &[FileRename]) {
    for item in &mut report.unresolved {
        if item.kind != UnresolvedKind::MissingPath {
            continue;
        }
        let path = item.spec.split('#').next().unwrap_or(&item.spec);
        if let Some(rename) = renames.iter().find(|rename| rename.from == path) {
            item.kind = UnresolvedKind::RenamedFile;
            item.detail = format!(
                "renamed {} → {} at {}; update the lens explicitly",
                rename.from, rename.to, rename.commit
            );
            item.renamed = Some(rename.clone());
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct LensReport {
    pub version: u32,
    pub lens_version: u32,
    pub title: Option<String>,
    pub repo: String,
    // Always true: see the module comment.
    pub lower_bound: bool,
    pub areas: Vec<ResolvedArea>,
    pub paths: Vec<ResolvedPath>,
    pub notes: Vec<ResolvedNote>,
    // In lens order, so the same lens and map always print the same list.
    pub unresolved: Vec<Unresolved>,
}

// What `GET /api/maps/{owner}/{repo}/lenses/{name}` returns.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct LensResponse {
    pub repository: String,
    pub commit: String,
    pub name: String,
    pub report: LensReport,
}

// What `GET /api/maps/{owner}/{repo}/lenses` returns.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct LensList {
    pub repository: String,
    pub commit: String,
    pub names: Vec<String>,
}

// ---- routes ----------------------------------------------------------------

type Adj = HashMap<usize, Vec<usize>>;

// Port of web/src/map/graph.ts `bfs`/`findRoute`. Same semantics on purpose:
// neighbours are visited in `E` order, a directed route is always preferred
// over a shorter undirected one (the direction is the fact worth stating),
// and the reversed direction is reported as `imported-by`. Keep the two in
// step; `tests/lens.rs` pins the cases where they could diverge.
fn bfs(from: usize, to: usize, graph: &Adj) -> Option<Vec<usize>> {
    if from == to {
        return Some(vec![from]);
    }
    let mut prev: HashMap<usize, Option<usize>> = HashMap::from([(from, None)]);
    let mut queue = vec![from];
    let mut head = 0;
    while head < queue.len() {
        let cur = queue[head];
        head += 1;
        for &next in graph.get(&cur).map(Vec::as_slice).unwrap_or_default() {
            if prev.contains_key(&next) {
                continue;
            }
            prev.insert(next, Some(cur));
            if next == to {
                let mut path = vec![to];
                let mut c = Some(cur);
                while let Some(n) = c {
                    path.push(n);
                    c = prev[&n];
                }
                path.reverse();
                return Some(path);
            }
            queue.push(next);
        }
    }
    None
}

pub struct Routes {
    adj: Adj,
    und: Adj,
}

impl Routes {
    pub fn new(map: &MapDocument) -> Routes {
        let mut adj = Adj::new();
        let mut und = Adj::new();
        for &[a, b] in &map.edges {
            adj.entry(a).or_default().push(b);
            und.entry(a).or_default().push(b);
            und.entry(b).or_default().push(a);
        }
        Routes { adj, und }
    }

    pub fn find(&self, a: usize, b: usize) -> Option<(Vec<usize>, RouteKind)> {
        if let Some(p) = bfs(a, b, &self.adj) {
            return Some((p, RouteKind::Imports));
        }
        if let Some(mut p) = bfs(b, a, &self.adj) {
            p.reverse();
            return Some((p, RouteKind::ImportedBy));
        }
        bfs(a, b, &self.und).map(|p| (p, RouteKind::Undirected))
    }
}

// ---- resolution ------------------------------------------------------------

struct Cx<'a> {
    map: &'a MapDocument,
    files: BTreeMap<&'a str, usize>,
    unresolved: Vec<Unresolved>,
}

impl Cx<'_> {
    fn target(&self, file: usize, symbol: Option<SymbolTarget>) -> LensTarget {
        LensTarget {
            file,
            path: self.map.files[file].clone(),
            district: self.map.nodes[file].district(),
            symbol,
        }
    }

    fn fail(&mut self, kind: UnresolvedKind, location: &str, spec: &str, detail: String) {
        self.unresolved.push(Unresolved {
            kind,
            location: location.to_owned(),
            spec: spec.to_owned(),
            detail,
            renamed: None,
        });
    }

    // Resolves one spec to its targets (several for a glob). On failure
    // returns nothing and records exactly one unresolved item.
    fn resolve_spec(&mut self, spec: &str, location: &str, glob_ok: bool) -> Vec<LensTarget> {
        match parse_spec(spec) {
            Spec::Glob(pattern) if glob_ok => {
                let hits: Vec<usize> = self
                    .files
                    .iter()
                    .filter(|(p, _)| glob_matches(pattern, p))
                    .map(|(_, &i)| i)
                    .collect();
                if hits.is_empty() {
                    self.fail(
                        UnresolvedKind::EmptyGlob,
                        location,
                        spec,
                        "no file of the map matches".into(),
                    );
                }
                hits.into_iter().map(|i| self.target(i, None)).collect()
            }
            Spec::Glob(_) => {
                self.fail(
                    UnresolvedKind::MissingPath,
                    location,
                    spec,
                    "a glob is not allowed here".into(),
                );
                vec![]
            }
            Spec::File(path) => match self.files.get(path) {
                Some(&i) => vec![self.target(i, None)],
                None => {
                    self.fail(
                        UnresolvedKind::MissingPath,
                        location,
                        spec,
                        "not a file of the map".into(),
                    );
                    vec![]
                }
            },
            Spec::Symbol { file, name, line } => {
                let Some(&i) = self.files.get(file) else {
                    self.fail(
                        UnresolvedKind::MissingPath,
                        location,
                        spec,
                        "not a file of the map".into(),
                    );
                    return vec![];
                };
                let rows = self
                    .map
                    .symbols
                    .get(&i.to_string())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let mut found: Vec<SymbolTarget> = rows
                    .iter()
                    .filter(|r| r.0 .0 == name && line.is_none_or(|l| r.0 .2 == l))
                    .map(|r| SymbolTarget {
                        name: r.0 .0.clone(),
                        kind: r.0 .1,
                        start: r.0 .2,
                        end: r.0 .3,
                    })
                    .collect();
                found.sort_by_key(|s| s.start);
                match found.len() {
                    0 => {
                        self.fail(
                            UnresolvedKind::UnknownSymbol,
                            location,
                            spec,
                            format!("{file} has no symbol {name:?} in the map"),
                        );
                        vec![]
                    }
                    1 => vec![self.target(i, found.pop())],
                    n => {
                        let starts: Vec<String> =
                            found.iter().map(|s| s.start.to_string()).collect();
                        self.fail(
                            UnresolvedKind::Ambiguous,
                            location,
                            spec,
                            format!(
                                "{n} symbols named {name:?}; add @<line>, one of {}",
                                starts.join(", ")
                            ),
                        );
                        vec![]
                    }
                }
            }
        }
    }

    fn resolve_one(&mut self, spec: &str, location: &str) -> Option<LensTarget> {
        self.resolve_spec(spec, location, false).pop()
    }
}

pub fn resolve(lens: &Lens, map: &MapDocument) -> LensReport {
    let mut cx = Cx {
        map,
        files: map
            .files
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_str(), i))
            .collect(),
        unresolved: vec![],
    };
    let routes = Routes::new(map);

    let areas = lens
        .areas
        .iter()
        .map(|area| {
            let mut members = vec![];
            for (j, spec) in area.members.iter().enumerate() {
                let location = format!("areas.{}.members[{j}]", area.label);
                for target in cx.resolve_spec(spec, &location, true) {
                    members.push(ResolvedMember {
                        spec: spec.clone(),
                        target,
                    });
                }
            }
            members.sort_by(|a, b| {
                let key = |m: &ResolvedMember| {
                    (
                        m.target.path.clone(),
                        m.target.symbol.as_ref().map(|s| (s.start, s.name.clone())),
                    )
                };
                key(a).cmp(&key(b)).then_with(|| a.spec.cmp(&b.spec))
            });
            members.dedup_by(|b, a| a.target == b.target);
            ResolvedArea {
                label: area.label.clone(),
                color: area.color,
                members,
            }
        })
        .collect();

    let paths = lens
        .paths
        .iter()
        .map(|path| {
            let stops: Vec<ResolvedStop> = path
                .stops
                .iter()
                .enumerate()
                .map(|(i, stop)| ResolvedStop {
                    spec: stop.at.clone(),
                    target: cx.resolve_one(&stop.at, &format!("paths.{}.stops[{i}]", path.name)),
                })
                .collect();
            let hops = (1..stops.len())
                .map(|to| {
                    let stop = &path.stops[to];
                    let ends = stops[to - 1].target.as_ref().zip(stops[to].target.as_ref());
                    let mut hop = ResolvedHop {
                        from: to - 1,
                        to,
                        source: match stop.hop {
                            HopMode::Computed => HopSource::Computed,
                            HopMode::Declared => HopSource::Declared,
                        },
                        edge_kind: None,
                        via: vec![],
                        reason: match stop.hop {
                            HopMode::Computed => None,
                            HopMode::Declared => stop.reason.clone(),
                        },
                        resolved: false,
                    };
                    let Some((a, b)) = ends else {
                        return hop;
                    };
                    if stop.hop == HopMode::Declared {
                        hop.resolved = true;
                        return hop;
                    }
                    let found = if a.file == b.file {
                        Some((vec![a.file], RouteKind::SameFile))
                    } else {
                        routes.find(a.file, b.file)
                    };
                    match found {
                        Some((files, kind)) => {
                            hop.resolved = true;
                            hop.edge_kind = Some(kind);
                            hop.via = files.into_iter().map(|f| map.files[f].clone()).collect();
                        }
                        None => cx.fail(
                            UnresolvedKind::NoRoute,
                            &format!("paths.{}.hops[{}]", path.name, to - 1),
                            &format!("{} -> {}", a.path, b.path),
                            "no route in the map's edges (lower bound; add `hop = \"declared\"` with a reason if the link exists)".into(),
                        ),
                    }
                    hop
                })
                .collect();
            ResolvedPath {
                name: path.name.clone(),
                stops,
                hops,
            }
        })
        .collect();

    let notes = lens
        .notes
        .iter()
        .enumerate()
        .map(|(i, note)| {
            let location = format!("notes[{i}].anchor");
            let mut out = ResolvedNote {
                anchor: note.anchor.clone(),
                text: note.text.clone(),
                district: None,
                target: None,
            };
            if let Some(name) = note.anchor.strip_prefix("district:") {
                let ids: Vec<usize> = map
                    .names
                    .iter()
                    .filter(|(_, n)| n.as_str() == name)
                    .filter_map(|(id, _)| id.parse().ok())
                    .collect();
                match ids.len() {
                    0 => cx.fail(
                        UnresolvedKind::UnknownDistrict,
                        &location,
                        &note.anchor,
                        "no district has that name in the map".into(),
                    ),
                    1 => out.district = Some(ids[0]),
                    n => cx.fail(
                        UnresolvedKind::Ambiguous,
                        &location,
                        &note.anchor,
                        format!("{n} districts share that name"),
                    ),
                }
            } else {
                out.target = cx.resolve_one(&note.anchor, &location);
            }
            out
        })
        .collect();

    LensReport {
        version: REPORT_VERSION,
        lens_version: lens.version,
        title: lens.title.clone(),
        repo: map.repo.clone(),
        lower_bound: true,
        areas,
        paths,
        notes,
        unresolved: cx.unresolved,
    }
}

pub fn render_json(report: &LensReport) -> String {
    let mut text = serde_json::to_string_pretty(report).expect("a LensReport always serialises");
    text.push('\n');
    text
}

pub fn render_text(report: &LensReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let members: usize = report.areas.iter().map(|a| a.members.len()).sum();
    let _ = writeln!(
        out,
        "lens {}: {} areas ({members} members), {} paths, {} notes",
        report.title.as_deref().unwrap_or("(untitled)"),
        report.areas.len(),
        report.paths.len(),
        report.notes.len()
    );
    for path in &report.paths {
        let _ = writeln!(out, "path {}", path.name);
        for hop in &path.hops {
            let how = match (hop.source, hop.edge_kind) {
                (HopSource::Declared, _) => "declared".to_owned(),
                (HopSource::Computed, Some(k)) => format!("computed ({})", route_name(k)),
                (HopSource::Computed, None) => "computed".to_owned(),
            };
            let status = if hop.resolved { "ok" } else { "UNRESOLVED" };
            let _ = writeln!(
                out,
                "  {} -> {}: {how}, {status}",
                path.stops[hop.from].spec, path.stops[hop.to].spec
            );
        }
    }
    if report.unresolved.is_empty() {
        out.push_str("all members, stops and anchors resolved (hops are lower bounds)\n");
    } else {
        let _ = writeln!(out, "{} unresolved:", report.unresolved.len());
        for u in &report.unresolved {
            let _ = writeln!(
                out,
                "  {} {} {:?}: {}",
                kind_name(u.kind),
                u.location,
                u.spec,
                u.detail
            );
        }
    }
    out
}

// `tolmap lens check`. Exit codes follow `tolmap check`: 0 clean, 1 findings,
// 2 input error.
pub fn run_check(lens: &std::path::Path, map: &std::path::Path, format: &str) -> i32 {
    let load = || -> Result<LensReport, String> {
        let text = std::fs::read_to_string(lens).map_err(|e| format!("{}: {e}", lens.display()))?;
        let lens = Lens::parse(&text).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(map).map_err(|e| format!("{}: {e}", map.display()))?;
        let map: MapDocument = serde_json::from_slice(&bytes)
            .map_err(|e| format!("{}: not a map document: {e}", map.display()))?;
        Ok(resolve(&lens, &map))
    };
    match load() {
        Ok(report) => {
            let rendered = if format == "json" {
                render_json(&report)
            } else {
                render_text(&report)
            };
            print!("{rendered}");
            i32::from(!report.unresolved.is_empty())
        }
        Err(message) => {
            eprintln!("tolmap lens check: {message}");
            2
        }
    }
}

fn route_name(kind: RouteKind) -> &'static str {
    match kind {
        RouteKind::Imports => "imports",
        RouteKind::ImportedBy => "imported-by",
        RouteKind::Undirected => "undirected",
        RouteKind::SameFile => "same-file",
    }
}

fn kind_name(kind: UnresolvedKind) -> &'static str {
    match kind {
        UnresolvedKind::MissingPath => "missing_path",
        UnresolvedKind::RenamedFile => "renamed_file",
        UnresolvedKind::EmptyGlob => "empty_glob",
        UnresolvedKind::UnknownSymbol => "unknown_symbol",
        UnresolvedKind::Ambiguous => "ambiguous",
        UnresolvedKind::UnknownDistrict => "unknown_district",
        UnresolvedKind::NoRoute => "no_route",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_semantics() {
        assert!(glob_matches("src/**/*.ts", "src/a/b/c.ts"));
        assert!(glob_matches("src/**/*.ts", "src/c.ts"));
        assert!(glob_matches("src/*.ts", "src/c.ts"));
        assert!(!glob_matches("src/*.ts", "src/a/c.ts"));
        assert!(glob_matches("a/?.rs", "a/x.rs"));
        assert!(!glob_matches("a/?.rs", "a/xy.rs"));
        assert!(glob_matches("**", "a/b"));
        assert!(glob_matches("src/**", "src/a/b.rs"));
        assert!(!glob_matches("src/**", "lib/a.rs"));
    }

    #[test]
    fn specs_classify() {
        assert_eq!(parse_spec("a/b.rs"), Spec::File("a/b.rs"));
        assert_eq!(parse_spec("a/*.rs"), Spec::Glob("a/*.rs"));
        assert_eq!(
            parse_spec("a/b.rs#f@12"),
            Spec::Symbol {
                file: "a/b.rs",
                name: "f",
                line: Some(12)
            }
        );
        assert_eq!(
            parse_spec("a/b.rs#f@x"),
            Spec::Symbol {
                file: "a/b.rs",
                name: "f@x",
                line: None
            }
        );
    }
}
