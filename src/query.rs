//! Compact CLI queries. These inspect stored maps; they never index a checkout.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use sha2::{Digest, Sha256};

use crate::query_contract::{
    DepsDirections, Direction, DirectionCounts, MapIdentity, Query, QueryCoverage, QueryEnvelope,
    QueryResponse, QueryResult, QuerySymbol, ReferenceMode, QUERY_SCHEMA,
};
use crate::schema::{MapDocument, SymbolsDocument};

#[derive(Clone, Debug, Args)]
pub struct QueryOptions {
    /// Map JSON or a build output directory containing exactly one map.
    #[arg(long, conflicts_with_all=["repo","commit","snapshot"])]
    pub map: Option<PathBuf>,
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long)]
    pub commit: Option<String>,
    #[arg(long)]
    pub snapshot: Option<String>,
    #[arg(long, default_value = "json", value_parser = ["json", "text"])]
    pub format: String,
    /// Maximum rows (deps: per direction; route: file steps); default 25, overview: all districts.
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct WhereArgs {
    pub target: String,
    #[command(flatten)]
    pub options: QueryOptions,
}

#[derive(Debug, Args)]
pub struct DepsArgs {
    pub path: String,
    #[arg(long, value_enum, default_value_t = Direction::Both)]
    pub direction: Direction,
    #[command(flatten)]
    pub options: QueryOptions,
}

#[derive(Debug, Args)]
pub struct RouteArgs {
    pub from: String,
    pub to: String,
    #[command(flatten)]
    pub options: QueryOptions,
}

// Levenshtein distance over characters; path text breaks equal-distance ties.
fn path_distance(a: &str, b: &str) -> usize {
    let b: Vec<_> = b.chars().collect();
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, left) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(above + 1)
                .min(diagonal + usize::from(left != *right));
            diagonal = above;
        }
    }
    row[b.len()]
}

pub struct Snapshot {
    pub map: MapDocument,
    pub identity: MapIdentity,
    path: PathBuf,
}

impl Snapshot {
    pub fn load(path: &Path) -> Result<Self> {
        let path = if path.is_dir() {
            let mut maps = Vec::new();
            for entry in std::fs::read_dir(path).context("read map directory")? {
                let candidate = entry?.path();
                let name = candidate.file_name().unwrap_or_default().to_string_lossy();
                if candidate.extension().is_some_and(|ext| ext == "json")
                    && !name.ends_with(".symbols.json")
                    && !name.ends_with(".names.json")
                    && candidate.is_file()
                {
                    let bytes = std::fs::read(&candidate)?;
                    if serde_json::from_slice::<MapDocument>(&bytes).is_ok() {
                        maps.push(candidate);
                    }
                }
            }
            if maps.len() != 1 {
                bail!("map directory must contain exactly one map JSON (found {}); pass its file explicitly", maps.len());
            }
            maps.remove(0)
        } else {
            path.to_owned()
        };
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        #[derive(serde::Deserialize)]
        struct StoredMap {
            #[serde(flatten)]
            map: MapDocument,
            #[serde(default)]
            commit: Option<String>,
        }
        let stored: StoredMap = serde_json::from_slice(&bytes).context("invalid map JSON")?;
        let map = stored.map;
        if map.files.len() != map.nodes.len()
            || map.edges.iter().flatten().any(|&i| i >= map.files.len())
            || map
                .nodes
                .iter()
                .any(|n| !map.names.contains_key(&n.district().to_string()))
            || map.files.iter().collect::<BTreeSet<_>>().len() != map.files.len()
        {
            bail!("invalid map file, district or edge indices");
        }
        let commit = stored.commit.or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit()))
                .map(str::to_owned)
        });
        let identity = MapIdentity {
            repo: map.repo.clone(),
            commit,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        Ok(Self {
            map,
            identity,
            path,
        })
    }

    fn district(&self, file: usize) -> String {
        self.map.names[&self.map.nodes[file].district().to_string()].clone()
    }

    fn coverage(&self, symbols: Option<&SymbolsDocument>) -> QueryCoverage {
        let references = self
            .map
            .coverage
            .as_ref()
            .and_then(|c| c.references.as_ref());
        let modes: BTreeSet<&str> = references
            .into_iter()
            .flat_map(|r| r.values().map(|r| r.path.as_str()))
            .collect();
        let refs = if modes.contains("scip") {
            if modes.len() == 1 {
                ReferenceMode::Scip
            } else {
                ReferenceMode::Mixed
            }
        } else {
            ReferenceMode::Hand
        };
        QueryCoverage {
            refs,
            zero_edge_files: self.map.coverage.as_ref().map(|c| c.zero_edge_files),
            calls_total: symbols.map(|s| s.coverage.calls_total),
            calls_resolved: symbols.map(|s| s.coverage.calls_resolved),
            unresolved: symbols.map(|s| s.coverage.unresolved.clone()),
            lower_bound: true,
        }
    }

    fn response(
        &self,
        query: Query,
        limit: usize,
        mut results: Vec<QueryResult>,
        symbols: Option<&SymbolsDocument>,
    ) -> QueryResponse {
        let total = results.len();
        results.truncate(limit);
        QueryEnvelope {
            schema: QUERY_SCHEMA.into(),
            query,
            map: self.identity.clone(),
            coverage: self.coverage(symbols),
            limit,
            total,
            truncated: total > limit,
            omitted: total.saturating_sub(limit),
            next_limit: (total > limit).then_some(total),
            results,
        }
    }

    pub fn where_is(&self, target: &str, limit: usize) -> Result<QueryResponse> {
        if let Some(file) = self.map.files.iter().position(|p| p == target) {
            return Ok(self.response(
                Query::Where {
                    target: target.into(),
                    ambiguous: false,
                    missing_path: None,
                    suggestions: Vec::new(),
                },
                limit,
                vec![QueryResult::File {
                    path: target.into(),
                    district: self.district(file),
                    symbol: None,
                }],
                None,
            ));
        }
        if !target.contains("::")
            && (target.contains('/') || Path::new(target).extension().is_some())
        {
            let mut nearest: Vec<_> = self
                .map
                .files
                .iter()
                .map(|path| (path_distance(target, path), path.clone()))
                .collect();
            nearest.sort();
            return Ok(self.response(
                Query::Where {
                    target: target.into(),
                    ambiguous: false,
                    missing_path: Some(target.into()),
                    suggestions: nearest.into_iter().take(3).map(|(_, path)| path).collect(),
                },
                limit,
                Vec::new(),
                None,
            ));
        }
        let symbols_path = if self.path.file_name().is_some_and(|name| name == "map.json")
            && self.path.with_file_name("symbols.json").is_file()
        {
            self.path.with_file_name("symbols.json")
        } else {
            self.path.with_extension("symbols.json")
        };
        let symbols: Option<SymbolsDocument> = match std::fs::read(&symbols_path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).context("invalid symbols JSON")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("read symbols JSON"),
        };
        let mut hits = Vec::new();
        if let Some(symbols) = &symbols {
            for (i, row) in symbols.symbols.iter().enumerate() {
                let (file, name, _, start, end, parent, _, _) = &row.0;
                if *file >= self.map.files.len() {
                    bail!("invalid symbols file index");
                }
                let mut parts = vec![name.as_str()];
                let mut parent = *parent;
                let mut seen = BTreeSet::from([i]);
                while parent >= 0 {
                    let p = parent as usize;
                    if !seen.insert(p) {
                        bail!("cycle in symbol parents");
                    }
                    let row = symbols
                        .symbols
                        .get(p)
                        .context("invalid symbol parent index")?;
                    if row.0 .0 != *file {
                        bail!("symbol parent belongs to another file");
                    }
                    parts.push(&row.0 .1);
                    parent = row.0 .5;
                }
                parts.reverse();
                let local = parts.join("::");
                let qualified = format!("{}::{local}", self.map.files[*file]);
                if target == name || target == local || target == qualified {
                    hits.push((
                        *file,
                        QuerySymbol {
                            name: name.clone(),
                            qualified_name: qualified,
                            start: *start,
                            end: *end,
                        },
                    ));
                }
            }
        } else {
            // Older maps carry only the compact S table. No parent hierarchy or
            // unresolved-call measurement can be recovered from that table.
            for (file, rows) in &self.map.symbols {
                let file: usize = file.parse().context("invalid compact symbol file index")?;
                let path = self
                    .map
                    .files
                    .get(file)
                    .context("invalid compact symbol file index")?;
                for row in rows {
                    let (name, _, start, end) = &row.0;
                    let qualified = format!("{path}::{name}");
                    if target == name || target == qualified {
                        hits.push((
                            file,
                            QuerySymbol {
                                name: name.clone(),
                                qualified_name: qualified,
                                start: *start,
                                end: *end,
                            },
                        ));
                    }
                }
            }
        }
        hits.sort_by(|(a, x), (b, y)| {
            self.map.files[*a]
                .cmp(&self.map.files[*b])
                .then(x.start.cmp(&y.start))
                .then(x.qualified_name.cmp(&y.qualified_name))
                .then(x.end.cmp(&y.end))
        });
        let ambiguous = hits.len() > 1;
        let results = hits
            .into_iter()
            .map(|(file, symbol)| QueryResult::File {
                path: self.map.files[file].clone(),
                district: self.district(file),
                symbol: Some(symbol),
            })
            .collect();
        Ok(self.response(
            Query::Where {
                target: target.into(),
                ambiguous,
                missing_path: None,
                suggestions: Vec::new(),
            },
            limit,
            results,
            symbols.as_ref(),
        ))
    }

    pub fn overview(&self, limit: usize) -> QueryResponse {
        let mut counts = std::collections::BTreeMap::<usize, (usize, usize, usize)>::new();
        for node in &self.map.nodes {
            counts.entry(node.district()).or_default().0 += 1;
        }
        for &[a, b] in &self.map.edges {
            let a = self.map.nodes[a].district();
            let b = self.map.nodes[b].district();
            if a != b {
                counts.entry(a).or_default().2 += 1;
                counts.entry(b).or_default().1 += 1;
            }
        }
        let mut districts: Vec<_> = counts.into_iter().collect();
        districts.sort_by(|(a, x), (b, y)| {
            y.0.cmp(&x.0)
                .then(self.map.names[&a.to_string()].cmp(&self.map.names[&b.to_string()]))
                .then(a.cmp(b))
        });
        let results = districts
            .into_iter()
            .map(
                |(id, (files, imports_in, imports_out))| QueryResult::District {
                    name: self.map.names[&id.to_string()].clone(),
                    files,
                    imports_in,
                    imports_out,
                },
            )
            .collect();
        self.response(
            Query::Overview {
                files: self.map.files.len(),
                imports: self.map.edges.len(),
                districts: self.map.districts.len(),
                language: self.map.lang.clone(),
                excluded_tests: self.map.excluded_tests,
                test_exclusion_rule: crate::extract::TEST_EXCLUSION_RULE.into(),
            },
            limit,
            results,
            None,
        )
    }

    pub fn route(&self, from: &str, to: &str, limit: usize) -> QueryResponse {
        let endpoints = self
            .map
            .files
            .iter()
            .position(|p| p == from)
            .zip(self.map.files.iter().position(|p| p == to));
        let route = endpoints.and_then(|(a, b)| {
            if a == b {
                Some((vec![a], crate::lens::RouteKind::SameFile))
            } else {
                crate::lens::Routes::new(&self.map).find(a, b)
            }
        });
        let edge_kind = route.as_ref().map(|(_, kind)| *kind);
        let results = route
            .into_iter()
            .flat_map(|(files, _)| files)
            .map(|file| QueryResult::Step {
                path: self.map.files[file].clone(),
                district: self.district(file),
            })
            .collect();
        self.response(
            Query::Route {
                from: from.into(),
                to: to.into(),
                edge_kind,
            },
            limit,
            results,
            None,
        )
    }

    pub fn deps(&self, path: &str, direction: Direction, limit: usize) -> QueryResponse {
        let mut hits = BTreeSet::new();
        if let Some(file) = self.map.files.iter().position(|p| p == path) {
            for &[a, b] in &self.map.edges {
                if a == file && direction != Direction::In {
                    hits.insert((self.map.files[b].clone(), 1u8, b));
                }
                if b == file && direction != Direction::Out {
                    hits.insert((self.map.files[a].clone(), 0u8, a));
                }
            }
        }
        let mut totals = [0usize; 2];
        for (_, d, _) in &hits {
            totals[*d as usize] += 1;
        }
        let counts = |total: usize| DirectionCounts {
            total,
            returned: total.min(limit),
            omitted: total.saturating_sub(limit),
        };
        let directions = DepsDirections {
            inbound: (direction != Direction::Out).then(|| counts(totals[0])),
            out: (direction != Direction::In).then(|| counts(totals[1])),
        };
        let mut returned = [0usize; 2];
        let results = hits
            .into_iter()
            .filter(|(_, d, _)| {
                let count = &mut returned[*d as usize];
                if *count >= limit {
                    return false;
                }
                *count += 1;
                true
            })
            .map(|(path, d, file)| QueryResult::Dependency {
                path,
                district: self.district(file),
                direction: if d == 0 {
                    Direction::In
                } else {
                    Direction::Out
                },
            })
            .collect();
        let total = totals.iter().sum();
        let omitted = total - returned.iter().sum::<usize>();
        QueryEnvelope {
            schema: QUERY_SCHEMA.into(),
            query: Query::Deps {
                path: path.into(),
                direction,
                directions,
            },
            map: self.identity.clone(),
            coverage: self.coverage(None),
            limit,
            total,
            truncated: omitted > 0,
            omitted,
            next_limit: (omitted > 0).then_some(totals[0].max(totals[1])),
            results,
        }
    }
}

pub fn run(options: &QueryOptions, query: impl FnOnce(&Snapshot) -> Result<QueryResponse>) -> i32 {
    run_with_data(options, None, query)
}
pub fn run_with_data(
    options: &QueryOptions,
    data: Option<&Path>,
    query: impl FnOnce(&Snapshot) -> Result<QueryResponse>,
) -> i32 {
    let snapshot = (|| -> Result<Snapshot> {
        if let Some(path) = options.map.as_deref() {
            return Snapshot::load(path);
        }
        let library = crate::personal::Library::open(data)?;
        let repository = library.resolve(options.repo.as_deref())?;
        let selected = library.snapshot(
            &repository,
            options.commit.as_deref(),
            options.snapshot.as_deref(),
        )?;
        let status = library.status(&repository)?;
        eprintln!(
            "{}: snapshot {} at {}; freshness={} dirty={}",
            repository.name, selected.id, selected.commit, status["freshness"], status["dirty"]
        );
        let mut loaded = Snapshot::load(&selected.map_path)?;
        loaded.identity.commit = Some(selected.commit);
        Ok(loaded)
    })();
    let report = match snapshot.and_then(|snapshot| query(&snapshot)) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("{e:#}");
            return 2;
        }
    };
    let output = if options.format == "json" {
        serde_json::to_string(&report).map(|s| format!("{s}\n"))
    } else {
        Ok(text(&report))
    };
    match output.map_err(anyhow::Error::from).and_then(|s| {
        std::io::stdout()
            .lock()
            .write_all(s.as_bytes())
            .map_err(Into::into)
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("write query response: {e}");
            3
        }
    }
}

pub fn text(report: &QueryResponse) -> String {
    let mut out = format!(
        "{}: {} of {} results{}\n",
        report.map.repo,
        report.results.len(),
        report.total,
        if report.truncated { " (truncated)" } else { "" }
    );
    if let Some(n) = report.next_limit {
        if let Query::Deps { directions, .. } = &report.query {
            for (direction, counts) in [("in", &directions.inbound), ("out", &directions.out)] {
                if let Some(counts) = counts {
                    out.push_str(&format!(
                        "{direction}: {} of {} results, {} rows omitted; repeat with --limit {n} to return all rows\n",
                        counts.returned, counts.total, counts.omitted
                    ));
                }
            }
        } else {
            out.push_str(&format!(
                "{} rows omitted; repeat with --limit {n} to return all rows\n",
                report.omitted
            ));
        }
    }
    out.push_str(&format!(
        "{} commit={} sha256={} limit={}\n",
        report.schema,
        report.map.commit.as_deref().unwrap_or("unknown"),
        report.map.sha256,
        report.limit
    ));
    out.push_str(&format!(
        "query={} coverage={}\n",
        serde_json::to_string(&report.query).expect("query serializes"),
        serde_json::to_string(&report.coverage).expect("coverage serializes")
    ));
    if matches!(
        report.query,
        Query::Where {
            ambiguous: true,
            ..
        }
    ) {
        out.push_str("ambiguous symbol: candidates follow\n");
    }
    match &report.query {
        Query::Status {} => {}
        Query::Where {
            missing_path: Some(path),
            suggestions,
            ..
        } => {
            out.push_str(&format!("path is not in the map: {path}\n"));
            if !suggestions.is_empty() {
                out.push_str("nearest mapped paths:\n");
                for suggestion in suggestions {
                    out.push_str(&format!("  {suggestion}\n"));
                }
            }
        }
        Query::Overview {
            files,
            imports,
            districts,
            language,
            excluded_tests,
            test_exclusion_rule,
        } => {
            out.push_str(&format!(
                "{files} files, {imports} imports, {districts} districts, language {language}\n"
            ));
            let count = excluded_tests.map_or_else(
                || "unknown (not recorded by this map)".into(),
                |n| n.to_string(),
            );
            out.push_str(&format!(
                "excluded test files: {count}; rule: {test_exclusion_rule}\n"
            ));
        }
        Query::Route { edge_kind, .. } => out.push_str(&format!(
            "file import path ({edge_kind:?}); not a runtime call chain\n"
        )),
        _ => {}
    }
    for result in &report.results {
        match result {
            QueryResult::File {
                path,
                district,
                symbol,
            } => {
                out.push_str(&format!("{path} [{district}]"));
                if let Some(s) = symbol {
                    out.push_str(&format!(" {}:{}-{}", s.qualified_name, s.start, s.end));
                }
                out.push('\n');
            }
            QueryResult::District {
                name,
                files,
                imports_in,
                imports_out,
            } => out.push_str(&format!(
                "{name}: {files} files, {imports_in} imports in, {imports_out} out\n"
            )),
            QueryResult::Step { path, district } => out.push_str(&format!("{path} [{district}]\n")),
            QueryResult::Dependency {
                path,
                district,
                direction,
            } => out.push_str(&format!(
                "{} {path} [{district}]\n",
                if *direction == Direction::In {
                    "in"
                } else {
                    "out"
                }
            )),
        }
    }
    out.push_str("Lower bound over extracted imports; unresolved counts are unknown unless symbols were loaded.\n");
    out
}
