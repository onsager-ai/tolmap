//! `tolmap lens suggest` (issue #249): starting lenses proposed from what a
//! map already knows, its landmarks and its edges. Each suggestion is an
//! ordinary lens file (format v1, `src/lens.rs`) for a human or agent to
//! review, edit and commit; nothing here is map state.
//!
//! Three lenses:
//!   - `entry-routes`: from each `entry` landmark, a walk through the
//!     highest-fan-in files it imports (directly or not), ending in another
//!     district where one is reachable;
//!   - `district-tour`: one area per district, with its `capital` landmark as
//!     a note anchor;
//!   - `crossings`: a path through each `bridge` landmark and one file edge
//!     along each of the heaviest cross-district roads.
//!
//! Every hop is `computed` and every stop is reachable from the previous one
//! by directed edges, so each hop resolves as `imports` in the same route
//! computation `tolmap lens check` uses (`lens::Routes`). A declared hop needs
//! a reason, and suggest cannot know one, so it never writes one.
//!
//! Nothing is randomised; every choice is a sort with a total tie-break
//! (fan-in descending, then path), so the same map gives byte-identical files.
use crate::lens::{Lens, LensArea, LensColor, LensNote, LensPath, LensStop, LENS_VERSION};
use crate::schema::MapDocument;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::Path;

// Hops in one entry route. A route ends earlier once it reaches another
// district or runs out of new files to import.
pub const ENTRY_MAX_HOPS: usize = 4;
// Heaviest cross-district roads drawn as crossing paths.
pub const CROSSING_ROADS: usize = 3;

pub const ENTRY_ROUTES: &str = "entry-routes";
pub const DISTRICT_TOUR: &str = "district-tour";
pub const CROSSINGS: &str = "crossings";

const PALETTE: [LensColor; 8] = [
    LensColor::Blue,
    LensColor::Teal,
    LensColor::Green,
    LensColor::Amber,
    LensColor::Orange,
    LensColor::Red,
    LensColor::Purple,
    LensColor::Grey,
];

pub struct Suggestions {
    // (lens name, file text), in a fixed order; a lens with nothing to
    // suggest is absent and explained in `messages`.
    pub lenses: Vec<(String, String)>,
    pub messages: Vec<String>,
}

// "Which map" for the header: its file name, its repository and the
// SHA-256 of its bytes. Not the path as typed, which would make the output
// depend on the working directory.
pub fn map_source(path: &Path, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{name} (sha256 {digest})")
}

struct Cx<'a> {
    map: &'a MapDocument,
    out: Vec<Vec<usize>>,
    into: Vec<Vec<usize>>,
}

impl<'a> Cx<'a> {
    fn new(map: &'a MapDocument) -> Self {
        let n = map.files.len();
        let mut out = vec![vec![]; n];
        let mut into = vec![vec![]; n];
        for &[a, b] in &map.edges {
            if a != b {
                out[a].push(b);
                into[b].push(a);
            }
        }
        Cx { map, out, into }
    }

    fn path(&self, f: usize) -> &'a str {
        &self.map.files[f]
    }

    fn district(&self, f: usize) -> usize {
        self.map.nodes[f].district()
    }

    // A path a lens can name as a plain file: `#` would read as a symbol
    // and `*`/`?` as a glob, so such a file is never suggested.
    fn usable(&self, f: usize) -> bool {
        let p = self.path(f);
        !p.contains(['#', '*', '?']) && crate::source::valid_path(p)
    }

    // The best of `files` by fan-in descending, then path.
    fn best(&self, files: impl IntoIterator<Item = usize>) -> Option<usize> {
        files
            .into_iter()
            .filter(|&f| self.usable(f))
            .min_by(|&a, &b| {
                let fa = self.map.nodes[a].fanin();
                let fb = self.map.nodes[b].fanin();
                fb.total_cmp(&fa)
                    .then_with(|| self.path(a).cmp(self.path(b)))
            })
    }

    // The best of `files` outside district `home`.
    fn best_away(&self, files: impl IntoIterator<Item = usize>, home: usize) -> Option<usize> {
        self.best(files.into_iter().filter(|&f| self.district(f) != home))
    }

    // Files reachable from `from` by directed edges, `from` excluded.
    fn reachable(&self, from: usize) -> BTreeSet<usize> {
        let mut seen = BTreeSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(cur) = queue.pop_front() {
            for &next in &self.out[cur] {
                if seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        seen.remove(&from);
        seen
    }

    fn district_name(&self, d: usize) -> String {
        self.map
            .names
            .get(&d.to_string())
            .cloned()
            .unwrap_or_else(|| format!("district {d}"))
    }

    // `district:<name>` when exactly one district has that name.
    fn district_anchor(&self, d: usize) -> Option<String> {
        let name = self.map.names.get(&d.to_string())?;
        let same = self.map.names.values().filter(|n| *n == name).count();
        (same == 1 && !name.is_empty()).then(|| format!("district:{name}"))
    }

    fn landmarks(&self, kind: &str) -> Vec<(usize, &'a str)> {
        let mut rows: Vec<_> = self
            .map
            .landmarks
            .iter()
            .filter(|l| l.0 .1 == kind && l.0 .0 < self.map.files.len())
            .map(|l| (l.0 .3, l.0 .0, l.0 .2.as_str()))
            .collect();
        rows.sort();
        rows.into_iter().map(|(_, f, detail)| (f, detail)).collect()
    }
}

fn stops(files: &[usize], cx: &Cx) -> Vec<LensStop> {
    files
        .iter()
        .map(|&f| LensStop {
            at: cx.path(f).to_owned(),
            hop: Default::default(),
            reason: None,
        })
        .collect()
}

// A path name ([a-z0-9_-], at most 64) from a prefix and file paths, so it
// names the files rather than a position in a list (lens comparison matches
// paths by name). A long name keeps a readable head and an FNV-1a hash of
// the whole; a repeat gets a counter.
fn path_name(prefix: &str, parts: &[&str], taken: &mut BTreeSet<String>) -> String {
    let mut raw = String::from(prefix);
    for part in parts {
        raw.push('-');
        raw.push_str(part);
    }
    let mut slug = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            c
        } else {
            '-'
        };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    let mut slug = slug.trim_end_matches('-').to_owned();
    if slug.len() > 64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in raw.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        slug = format!("{}-{hash:016x}", slug[..47].trim_end_matches('-'));
    }
    let mut name = slug.clone();
    let mut n = 2;
    while !taken.insert(name.clone()) {
        name = format!("{}-{n}", &slug[..slug.len().min(60)]);
        n += 1;
    }
    name
}

fn header(kind: &str, about: &str, source: &str, repo: &str) -> String {
    format!(
        "# Generated by `tolmap lens suggest` ({kind}) from map {source},\n\
         # repository {repo}. {about}\n\
         # Every hop is computed from the map's file-level edges, so each route is a\n\
         # lower bound: a link the map cannot see (dynamic dispatch, a queue, a\n\
         # fetch) is absent, not disproven. Review and edit before committing it.\n"
    )
}

fn entry_routes(cx: &Cx, names: &mut BTreeSet<String>) -> (Option<Lens>, Vec<String>) {
    let entries = cx.landmarks("entry");
    if entries.is_empty() {
        return (
            None,
            vec![format!(
                "no entry landmark in the map: {ENTRY_ROUTES} not written"
            )],
        );
    }
    let mut paths = vec![];
    let mut notes = vec![];
    let mut messages = vec![];
    for (entry, detail) in entries {
        if !cx.usable(entry) {
            messages.push(format!(
                "entry {:?} cannot be named in a lens; no route",
                cx.path(entry)
            ));
            continue;
        }
        let home = cx.district(entry);
        let mut route = vec![entry];
        let mut seen = BTreeSet::from([entry]);
        while route.len() <= ENTRY_MAX_HOPS {
            let cur = *route.last().unwrap();
            let ahead: Vec<usize> = cx
                .reachable(cur)
                .into_iter()
                .filter(|f| !seen.contains(f))
                .collect();
            let Some(mut next) = cx.best(ahead.iter().copied()) else {
                break;
            };
            // Leave for another district now if this is the last hop, or if
            // the greedy pick would lose every other-district file still
            // reachable from here.
            if cx.district(next) == home {
                if let Some(other) = cx.best_away(ahead.iter().copied(), home) {
                    let last = route.len() == ENTRY_MAX_HOPS;
                    let strands = cx
                        .best_away(
                            cx.reachable(next).into_iter().filter(|f| !seen.contains(f)),
                            home,
                        )
                        .is_none();
                    if last || strands {
                        next = other;
                    }
                }
            }
            route.push(next);
            seen.insert(next);
            if cx.district(next) != home {
                break;
            }
        }
        if route.len() < 2 {
            messages.push(format!(
                "entry {:?} imports nothing in the map's edges; no route",
                cx.path(entry)
            ));
            continue;
        }
        let end = *route.last().unwrap();
        let ending = if cx.district(end) != home {
            format!("ends in district {}", cx.district_name(cx.district(end)))
        } else {
            format!(
                "stays in district {}: no other district is reachable by imports",
                cx.district_name(home)
            )
        };
        notes.push(LensNote {
            anchor: cx.path(entry).to_owned(),
            text: format!(
                "entry landmark ({detail}); the route follows the highest-fan-in files it imports and {ending}"
            ),
        });
        paths.push(LensPath {
            name: path_name("entry", &[cx.path(entry)], names),
            stops: stops(&route, cx),
        });
    }
    if paths.is_empty() {
        messages.push(format!(
            "no entry landmark imports anything in the map's edges: {ENTRY_ROUTES} not written"
        ));
        return (None, messages);
    }
    let lens = Lens {
        version: LENS_VERSION,
        title: Some("Entry routes (generated; lower bounds)".into()),
        areas: vec![],
        paths,
        notes,
    };
    (Some(lens), messages)
}

fn district_tour(cx: &Cx) -> Option<Lens> {
    let mut by_district: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for f in 0..cx.map.files.len() {
        if cx.usable(f) {
            by_district.entry(cx.district(f)).or_default().push(f);
        }
    }
    // Largest district first, then by id.
    let mut districts: Vec<(usize, Vec<usize>)> = by_district.into_iter().collect();
    districts.sort_by_key(|(d, files)| (std::cmp::Reverse(files.len()), *d));
    let capitals: HashMap<usize, (usize, &str)> = cx
        .landmarks("capital")
        .into_iter()
        .map(|(f, detail)| (cx.district(f), (f, detail)))
        .rev()
        .collect();
    let mut areas = vec![];
    let mut notes = vec![];
    let mut labels = BTreeSet::new();
    for (i, (d, mut files)) in districts.into_iter().enumerate() {
        let name = cx.district_name(d);
        let mut label = name.clone();
        if !labels.insert(label.clone()) {
            label = format!("{name} (district {d})");
            labels.insert(label.clone());
        }
        files.sort_by_key(|&f| cx.path(f));
        let count = files.len();
        areas.push(LensArea {
            label,
            color: PALETTE[i % PALETTE.len()],
            members: files.iter().map(|&f| cx.path(f).to_owned()).collect(),
        });
        match capitals.get(&d) {
            Some(&(capital, _)) if cx.usable(capital) => notes.push(LensNote {
                anchor: cx.path(capital).to_owned(),
                text: format!(
                    "capital of district {name} ({count} files): its most connected file; start reading the district here"
                ),
            }),
            _ => {
                if let Some(anchor) = cx.district_anchor(d) {
                    notes.push(LensNote {
                        anchor,
                        text: format!("district {name} ({count} files) has no capital landmark"),
                    });
                }
            }
        }
    }
    (!areas.is_empty()).then(|| Lens {
        version: LENS_VERSION,
        title: Some("District tour (generated)".into()),
        areas,
        paths: vec![],
        notes,
    })
}

fn crossings(cx: &Cx, names: &mut BTreeSet<String>) -> (Option<Lens>, Vec<String>) {
    let mut paths = vec![];
    let mut notes = vec![];
    let mut messages = vec![];
    for (bridge, detail) in cx.landmarks("bridge") {
        if !cx.usable(bridge) {
            continue;
        }
        let home = cx.district(bridge);
        // Prefer neighbours in another district: the crossing is the point.
        let pick = |files: &[usize]| {
            cx.best_away(files.iter().copied(), home)
                .or_else(|| cx.best(files.iter().copied()))
        };
        let before = pick(&cx.into[bridge]);
        let after = pick(
            &cx.out[bridge]
                .iter()
                .copied()
                .filter(|&f| Some(f) != before)
                .collect::<Vec<_>>(),
        );
        let route: Vec<usize> = before.into_iter().chain([bridge]).chain(after).collect();
        if route.len() < 2 {
            messages.push(format!(
                "bridge {:?} has no edge a lens can name; no crossing",
                cx.path(bridge)
            ));
            continue;
        }
        notes.push(LensNote {
            anchor: cx.path(bridge).to_owned(),
            text: format!(
                "bridge landmark ({detail}); shown between its highest-fan-in importer and import, preferring other districts"
            ),
        });
        paths.push(LensPath {
            name: path_name("bridge", &[cx.path(bridge)], names),
            stops: stops(&route, cx),
        });
    }

    let mut roads: Vec<(usize, usize, f64)> = cx
        .map
        .roads
        .iter()
        .map(|r| (r.0 .0.min(r.0 .1), r.0 .0.max(r.0 .1), r.0 .2))
        .filter(|r| r.0 != r.1)
        .collect();
    roads.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| (a.0, a.1).cmp(&(b.0, b.1)))
    });
    for (a, b, weight) in roads.into_iter().take(CROSSING_ROADS) {
        // The file edge between the two districts whose files carry the
        // most fan-in, then by paths.
        let edge = cx
            .map
            .edges
            .iter()
            .filter(|&&[x, y]| {
                let pair = (cx.district(x), cx.district(y));
                (pair == (a, b) || pair == (b, a)) && cx.usable(x) && cx.usable(y)
            })
            .min_by(|&&[x1, y1], &&[x2, y2]| {
                let fan = |x: usize, y: usize| cx.map.nodes[x].fanin() + cx.map.nodes[y].fanin();
                fan(x2, y2)
                    .total_cmp(&fan(x1, y1))
                    .then_with(|| (cx.path(x1), cx.path(y1)).cmp(&(cx.path(x2), cx.path(y2))))
            });
        let (na, nb) = (cx.district_name(a), cx.district_name(b));
        let Some(&[x, y]) = edge else {
            messages.push(format!(
                "road {na} – {nb} (weight {weight}) has no file edge in the map; no crossing"
            ));
            continue;
        };
        notes.push(LensNote {
            anchor: cx.path(x).to_owned(),
            text: format!(
                "road {na} – {nb}, weight {weight}: one of its file edges, chosen by fan-in"
            ),
        });
        paths.push(LensPath {
            name: path_name("road", &[cx.path(x), cx.path(y)], names),
            stops: stops(&[x, y], cx),
        });
    }
    if paths.is_empty() {
        messages.push(format!(
            "no bridge landmark or cross-district road with a file edge: {CROSSINGS} not written"
        ));
        return (None, messages);
    }
    let lens = Lens {
        version: LENS_VERSION,
        title: Some("Crossings (generated; lower bounds)".into()),
        areas: vec![],
        paths,
        notes,
    };
    (Some(lens), messages)
}

pub fn suggest(map: &MapDocument, source: &str) -> Suggestions {
    let cx = Cx::new(map);
    let mut lenses = vec![];
    let mut messages = vec![];
    let mut emit = |name: &str, about: &str, lens: Option<Lens>| {
        if let Some(lens) = lens {
            let text = header(name, about, source, &map.repo) + &lens.to_toml();
            lenses.push((name.to_owned(), text));
        }
    };
    // Path names are unique per lens; one set per lens.
    let (lens, m) = entry_routes(&cx, &mut BTreeSet::new());
    messages.extend(m);
    emit(
        ENTRY_ROUTES,
        "From each entry landmark, a walk through the\n# highest-fan-in files it imports, ending in another district where one is\n# reachable.",
        lens,
    );
    emit(
        DISTRICT_TOUR,
        "One area per district, with its capital landmark\n# as a note anchor.",
        district_tour(&cx),
    );
    let (lens, m) = crossings(&cx, &mut BTreeSet::new());
    messages.extend(m);
    emit(
        CROSSINGS,
        "The bridge landmarks between their neighbours, and\n# one file edge along each of the heaviest cross-district roads.",
        lens,
    );
    if lenses.is_empty() {
        messages.push("nothing to suggest: the map has no landmarks, districts or edges".into());
    }
    Suggestions { lenses, messages }
}

// `tolmap lens suggest`. Writes `<name>.toml` files into `--out` (created if
// absent); without it, prints them to stdout and writes nothing, so it never
// writes into an analysed repository unasked. Messages go to stderr. Exits
// 0, or 2 when the map cannot be read or a file cannot be written.
pub fn run(map: &Path, out: Option<&Path>) -> i32 {
    let load = || -> Result<Suggestions, String> {
        let bytes = std::fs::read(map).map_err(|e| format!("{}: {e}", map.display()))?;
        let doc: MapDocument = serde_json::from_slice(&bytes)
            .map_err(|e| format!("{}: not a map document: {e}", map.display()))?;
        Ok(suggest(&doc, &map_source(map, &bytes)))
    };
    let suggestions = match load() {
        Ok(s) => s,
        Err(message) => {
            eprintln!("tolmap lens suggest: {message}");
            return 2;
        }
    };
    for message in &suggestions.messages {
        eprintln!("tolmap lens suggest: {message}");
    }
    match out {
        Some(dir) => {
            let write = || -> std::io::Result<()> {
                std::fs::create_dir_all(dir)?;
                for (name, text) in &suggestions.lenses {
                    let file = dir.join(format!("{name}.toml"));
                    std::fs::write(&file, text)?;
                    println!("{}", file.display());
                }
                Ok(())
            };
            if let Err(e) = write() {
                eprintln!("tolmap lens suggest: {}: {e}", dir.display());
                return 2;
            }
        }
        None => {
            for (i, (name, text)) in suggestions.lenses.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                println!("# ==> {name}.toml <==");
                print!("{text}");
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_names_are_valid_and_unique() {
        let mut taken = BTreeSet::new();
        assert_eq!(
            path_name("entry", &["src/Flask/app.py"], &mut taken),
            "entry-src-flask-app-py"
        );
        assert_eq!(
            path_name("entry", &["src/flask/app.py"], &mut taken),
            "entry-src-flask-app-py-2"
        );
        let long = "a/".repeat(60) + "x.py";
        let name = path_name("road", &[&long, "b.py"], &mut taken);
        assert!(name.len() <= 64 && name.starts_with("road-a-a-"), "{name}");
    }
}
