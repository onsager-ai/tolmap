use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::blobs;
use crate::extract::{self, round_to, LanguageKind};
use crate::naming;
use crate::parcels;
use crate::parity;
use crate::partition::LeidenFfi;
use crate::pipeline;
use crate::schema::{
    District, GraphData, LandmarkRow, MapDocument, NodeRow, SymbolRow, TerrainArterial,
    TerrainDistrict, TerrainParcel, TerrainSubdistrict,
};
use crate::terrain;

#[derive(Clone, Copy, Debug)]
pub struct BuildFeatures {
    pub parcels: bool,
    pub terrain: bool,
}

pub fn build(
    repo: &Path,
    pkg: &str,
    lang: &str,
    name: Option<&str>,
    out: &Path,
    resolution: f64,
    features: BuildFeatures,
) -> Result<PathBuf> {
    let language = LanguageKind::parse(lang)?;
    let map_name = name.map(str::to_owned).unwrap_or_else(|| {
        repo.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    eprintln!("[1/5] extract   {}/{}  ({lang})", repo.display(), pkg);
    let graph = extract::build(repo, pkg, language)?;
    build_from_graph(graph, map_name, out, resolution, features)
}

/// As [`build`], but unions any number of `(pkg, language)` sources (see
/// `extract::build_multi_source`) instead of parsing one -- the CLI entry
/// point for `tolmap build --pkg . --lang go --pkg web/ui --lang ts` and
/// `tolmap build --all-sources`.
pub fn build_multi(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    name: Option<&str>,
    out: &Path,
    resolution: f64,
    features: BuildFeatures,
) -> Result<PathBuf> {
    let map_name = name.map(str::to_owned).unwrap_or_else(|| {
        repo.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let described = sources
        .iter()
        .map(|(pkg, language)| format!("{pkg} ({})", language.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!(
        "[1/5] extract   {}  {} sources: {described}",
        repo.display(),
        sources.len()
    );
    let graph = extract::build_multi_source(repo, sources)?;
    build_from_graph(graph, map_name, out, resolution, features)
}

/// Runs the pipeline (partition, naming, geometry, parcels) against an
/// already-extracted graph, skipping the repository-parsing step `build`
/// does first. This is what lets CI exercise blend -> prune -> partition ->
/// parity on a pre-extracted graph (`tolmap dump-graph`'s output) without
/// cloning the source repository -- see the module doc on `schema::GraphData`.
///
/// Cold-start only (no warm start). Kept as the CLI's entry point rather
/// than adding an `Option` parameter to it directly so `tolmap build`'s
/// signature does not grow a job-service concern it cannot supply (the CLI
/// has no store to read a previous commit's membership from). The job
/// service calls [`build_from_graph_warm`] instead.
pub fn build_from_graph(
    graph: GraphData,
    map_name: String,
    out: &Path,
    resolution: f64,
    features: BuildFeatures,
) -> Result<PathBuf> {
    build_from_graph_warm(graph, map_name, out, resolution, features, None)
}

/// As [`build_from_graph`], but a prior [`MapDocument`] both seeds the
/// top-level partitioner and preserves matched terrain suffixes.  The job
/// service already has that document at this call site, so the terrain warm
/// path needs no new store or lookup plumbing.
pub fn build_from_graph_warm(
    graph: GraphData,
    map_name: String,
    out: &Path,
    resolution: f64,
    features: BuildFeatures,
    previous_document: Option<&MapDocument>,
) -> Result<PathBuf> {
    eprintln!("[2/5] partition resolution={resolution}");
    let partitioner = LeidenFfi;
    let previous_membership = previous_document.map(|document| {
        document
            .files
            .iter()
            .cloned()
            .zip(document.nodes.iter().map(NodeRow::district))
            .collect::<BTreeMap<_, _>>()
    });
    let initial = previous_membership
        .as_ref()
        .map(|prev| pipeline::align_initial_membership(&graph, prev));
    let layout = pipeline::run(graph, resolution, &partitioner, initial.as_deref())?;
    eprintln!("[3/5] name     districts");
    // Same convention `cli.py::build` uses: the cache lives next to the map
    // it names, `<out>/<name>.names.json`, so a rerun into the same --out
    // finds it with no extra flag. `eval/seed_names.py` writes this same
    // shape, keyed by the same fingerprint, to seed a fixture's committed
    // names into a fresh build directory.
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    let names_cache_path = out.join(format!("{map_name}.names.json"));
    let files = layout
        .weighted
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let (names, names_cache) =
        naming::name_districts(&files, &layout.membership, Some(&names_cache_path));
    naming::save_cache(&names_cache_path, &names_cache)
        .with_context(|| format!("write district names cache {}", names_cache_path.display()))?;
    for (district, district_name) in &names {
        let count = layout
            .membership
            .iter()
            .filter(|value| value.to_string() == *district)
            .count();
        eprintln!("        d{district:<2} {count:4} files  {district_name}");
    }
    eprintln!("[4/5] geometry regions");
    let geometry = blobs::build_geometry(&layout, &partitioner, features.terrain)?;
    let mut document = compact(map_name, layout, geometry, names, previous_document);
    if features.parcels {
        eprintln!("[5/5] geometry weighted-voronoi plots");
        document.parcels = Some(parcels::build_parcels(&document));
        if let Some(correlation) = parcels::area_correlation(&document) {
            eprintln!(
                "        parcels={}/{}  area~loc r={correlation:.3}",
                document.parcels.as_ref().map_or(0, BTreeMap::len),
                document.files.len()
            );
        }
    }
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    let output = out.join(format!("{}.json", document.repo));
    let bytes = serde_json::to_vec(&document)?;
    let temporary = output.with_extension("json.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &output)?;
    eprintln!("\nwrote {}", output.display());
    Ok(output)
}

fn compact(
    name: String,
    layout: pipeline::PipelineOutput,
    geometry: blobs::BlobGeometry,
    names: BTreeMap<String, String>,
    previous_document: Option<&MapDocument>,
) -> MapDocument {
    let files = layout
        .weighted
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let file_index = files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let nodes = layout
        .weighted
        .nodes
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let point = geometry.points[index];
            let rect = layout.nodes[index].rect;
            NodeRow((
                layout.membership[index],
                round_to(point[0], 4),
                round_to(point[1], 4),
                source.loc,
                source.complexity,
                source.churn,
                source.fanin,
                round_to(rect[0], 5),
                round_to(rect[1], 5),
                round_to(rect[2], 5),
                round_to(rect[3], 5),
            ))
        })
        .collect::<Vec<_>>();
    let edges = layout
        .weighted
        .imports
        .iter()
        .filter_map(|(a, b, _)| Some([*file_index.get(a.as_str())?, *file_index.get(b.as_str())?]))
        .collect::<Vec<_>>();
    let landmarks = layout
        .landmarks
        .iter()
        .map(|landmark| {
            LandmarkRow((
                landmark.node,
                landmark.why.clone(),
                landmark.detail.clone(),
                landmark.rank,
            ))
        })
        .collect::<Vec<_>>();
    let symbols = layout
        .weighted
        .symbols
        .iter()
        .filter_map(|(file, rows)| {
            let index = *file_index.get(file.as_str())?;
            let rows = rows
                .iter()
                .map(|row| {
                    SymbolRow((
                        row.0 .0.chars().take(44).collect(),
                        row.0 .1,
                        row.0 .2,
                        row.0 .3,
                    ))
                })
                .collect::<Vec<_>>();
            (!rows.is_empty()).then_some((index.to_string(), rows))
        })
        .collect::<BTreeMap<_, _>>();
    let uses = compact_uses(&layout, &file_index);
    let terrain = geometry.terrain.as_ref().map(|districts| {
        // Parent matching exists only to warm terrain suffixes. Keep it
        // inside this branch so the service's default terrain-off path does
        // no new matching work; the prior document still seeds the existing
        // top-level warm partition above.
        let previous_district_matches = previous_document.map(|previous| {
            let candidate = files
                .iter()
                .cloned()
                .zip(layout.membership.iter().copied())
                .collect::<BTreeMap<_, _>>();
            let reference = previous
                .files
                .iter()
                .cloned()
                .zip(previous.nodes.iter().map(NodeRow::district))
                .collect::<BTreeMap<_, _>>();
            let common = candidate
                .keys()
                .filter(|file| reference.contains_key(*file))
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            parity::match_districts(&candidate, &reference, &common)
        });
        districts
            .iter()
            .map(|(&district, district_geometry)| {
                let current_files = district_geometry
                    .subdistricts
                    .iter()
                    .map(|subdistrict| {
                        subdistrict
                            .members
                            .iter()
                            .map(|&file| files[file].clone())
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                let previous_district_id = previous_district_matches
                    .as_ref()
                    .and_then(|matches| matches.get(&district))
                    .map(|(previous, _)| *previous);
                let previous_district = previous_document
                    .and_then(|document| document.terrain.as_ref())
                    .and_then(|terrain| terrain.get(&previous_district_id?.to_string()));
                let previous_groups = previous_district.map(|previous| {
                    previous
                        .subdistricts
                        .iter()
                        .map(|subdistrict| {
                            (
                                subdistrict.suffix,
                                subdistrict
                                    .members
                                    .iter()
                                    .filter_map(|&file| {
                                        previous_document
                                            .and_then(|document| document.files.get(file))
                                            .cloned()
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>()
                });
                let (suffixes, max_suffix) = terrain::assign_suffixes(
                    &current_files,
                    previous_groups.as_deref(),
                    previous_district.map_or(0, |previous| previous.max_suffix),
                );
                let mut subdistricts = district_geometry
                    .subdistricts
                    .iter()
                    .zip(suffixes)
                    .map(|(subdistrict, suffix)| TerrainSubdistrict {
                        suffix,
                        members: subdistrict.members.clone(),
                        c: [
                            round_to(subdistrict.center[0], 4),
                            round_to(subdistrict.center[1], 4),
                        ],
                        blob: subdistrict
                            .blob
                            .iter()
                            .map(|polygon| {
                                polygon
                                    .iter()
                                    .map(|point| [round_to(point[0], 4), round_to(point[1], 4)])
                                    .collect()
                            })
                            .collect(),
                    })
                    .collect::<Vec<_>>();
                subdistricts.sort_by_key(|subdistrict| subdistrict.suffix);
                (
                    district.to_string(),
                    TerrainDistrict {
                        arterials: district_geometry
                            .arterials
                            .iter()
                            .map(|(file, stranded, links)| TerrainArterial {
                                file: *file,
                                stranded: *stranded,
                                links: links.clone(),
                            })
                            .collect(),
                        subdistricts,
                        parcels: district_geometry
                            .parcels
                            .iter()
                            .map(|parcel| TerrainParcel {
                                address: parcel.address.clone(),
                                members: parcel.members.clone(),
                                rect: [
                                    round_to(parcel.rect[0], 4),
                                    round_to(parcel.rect[1], 4),
                                    round_to(parcel.rect[2], 4),
                                    round_to(parcel.rect[3], 4),
                                ],
                            })
                            .collect(),
                        max_suffix,
                    },
                )
            })
            .collect()
    });
    let mut groups = BTreeMap::<usize, Vec<usize>>::new();
    for (index, &district) in layout.membership.iter().enumerate() {
        groups.entry(district).or_default().push(index);
    }
    let districts = groups
        .into_iter()
        .map(|(district, members)| {
            let center = geometry.centroids[&district];
            let blob = geometry
                .blobs
                .get(&district)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|polygon| {
                    polygon
                        .into_iter()
                        .map(|point| [round_to(point[0], 4), round_to(point[1], 4)])
                        .collect()
                })
                .collect();
            (
                district.to_string(),
                District {
                    size: members.len(),
                    c: [round_to(center[0], 4), round_to(center[1], 4)],
                    blob,
                },
            )
        })
        .collect();
    MapDocument {
        repo: name,
        q: layout.modularity,
        names,
        districts,
        files,
        nodes,
        edges,
        landmarks,
        symbols,
        uses,
        roads: geometry.roads,
        lang: layout.weighted.lang,
        parcels: None,
        terrain,
    }
}

fn compact_uses(
    layout: &pipeline::PipelineOutput,
    file_index: &BTreeMap<&str, usize>,
) -> BTreeMap<String, Vec<usize>> {
    let mut symbol_index = BTreeMap::<usize, BTreeMap<String, usize>>::new();
    for (file, symbols) in &layout.weighted.symbols {
        let Some(&index) = file_index.get(file.as_str()) else {
            continue;
        };
        let mut names = BTreeMap::new();
        for (symbol_index, symbol) in symbols.iter().enumerate() {
            names.entry(symbol.0 .0.clone()).or_insert(symbol_index);
            let base = symbol.0 .0.rsplit('.').next().unwrap_or(&symbol.0 .0);
            names.entry(base.to_owned()).or_insert(symbol_index);
        }
        symbol_index.insert(index, names);
    }
    let raw = layout
        .weighted
        .uses
        .iter()
        .filter_map(|(a, b, name)| {
            Some((
                *file_index.get(a.as_str())?,
                *file_index.get(b.as_str())?,
                name.clone(),
            ))
        })
        .collect::<Vec<_>>();
    let mut outgoing = BTreeMap::<(usize, String), BTreeSet<usize>>::new();
    for (source, target, name) in &raw {
        outgoing
            .entry((*source, name.clone()))
            .or_default()
            .insert(*target);
    }
    let mut cache = BTreeMap::<(usize, String), Option<(usize, usize)>>::new();
    let mut result = BTreeMap::<String, BTreeSet<usize>>::new();
    for (source, target, name) in raw {
        let key = (target, name.clone());
        let definition = cache
            .entry(key)
            .or_insert_with(|| define_site(target, &name, &symbol_index, &outgoing, 4));
        if let Some((file, symbol)) = *definition {
            if file != source {
                result
                    .entry(format!("{file}:{symbol}"))
                    .or_default()
                    .insert(source);
            }
        }
    }
    result
        .into_iter()
        .filter(|(_, users)| !users.is_empty())
        .map(|(key, users)| (key, users.into_iter().collect()))
        .collect()
}

fn define_site(
    start: usize,
    name: &str,
    symbols: &BTreeMap<usize, BTreeMap<String, usize>>,
    outgoing: &BTreeMap<(usize, String), BTreeSet<usize>>,
    hops: usize,
) -> Option<(usize, usize)> {
    let mut seen = BTreeSet::new();
    let mut frontier = vec![start];
    for _ in 0..=hops {
        let mut next = Vec::new();
        for current in frontier {
            if !seen.insert(current) {
                continue;
            }
            if let Some(symbol) = symbols.get(&current).and_then(|table| table.get(name)) {
                return Some((current, *symbol));
            }
            if let Some(targets) = outgoing.get(&(current, name.to_owned())) {
                next.extend(targets.iter().copied());
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    None
}
