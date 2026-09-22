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
use crate::pipeline::{self, LayoutDistrict};
use crate::schema::{
    District, DistrictClass, FileId, GraphData, LandmarkRow, MapDocument, NodeRow, SymbolRow,
    TerrainArterial, TerrainDistrict, TerrainParcel, TerrainSubdistrict,
};
use crate::terrain;

#[derive(Clone, Copy, Debug)]
pub struct BuildFeatures {
    pub parcels: bool,
    pub terrain: bool,
    pub prune_variant: pipeline::PruneVariant,
}

/// A district holding at least this share of the repo's files is
/// **mainland**; a smaller district is either an **island** (it kept at
/// least one edge -- see [`classify_districts`]) or **unconnected** (it
/// kept none). Measured on three real repositories (issue #34, finding
/// 17), at `cb04469` --
///
/// ```text
/// crawlab    575 files ->  11 districts, 10 mainland /  0 islands /  1 unconnected
/// dify     6,335 files -> 136 districts, 19 mainland / 26 islands / 91 unconnected
/// n8n     11,982 files ->  85 districts, 15 mainland / 52 islands / 18 unconnected
/// ```
///
/// (1% of crawlab is ~6 files; 1% of n8n is ~120.) A share rather than a
/// fixed file count was chosen when the mainland count came out roughly
/// flat (16/27/28) across that 21x range, before finding 15's
/// module-resolution fix. It no longer is -- n8n now has fewer mainland
/// districts than dify, because finding 15 concentrated it into a few very
/// large communities -- so the evidence for 1% *specifically* is weaker
/// than it was; mainland still holds 86-99.8% of each repo's files.
/// Finding 17 records both corpora and what is not settled.
///
/// Expressed as an integer percentage rather than an `f64` share so the
/// boundary comparison (`size * 100 >= total * MAINLAND_SHARE_PERCENT`) is
/// exact on every platform -- a district sitting exactly on 1% classifies
/// the same way regardless of how `1.0 / 100.0` happens to round, rather
/// than relying on that rounding matching a literal `0.01` bit for bit.
const MAINLAND_SHARE_PERCENT: usize = 1;

/// Classifies every district in `membership` against
/// [`MAINLAND_SHARE_PERCENT`], using `imports` -- the resolved static-import
/// pairs (file indices) `data.imports` carries, the same edges `compact()`
/// turns into the map's own `E` -- to decide whether a below-threshold
/// district is an island or unconnected: a file counts as connected if it
/// is incident to at least one import edge, intra- or inter-district both,
/// per issue #34's definition.
///
/// Not `layout.graph` (the blended, pruned, per-signal-weighted graph the
/// partitioner actually ran on): that graph folds in cochange, proximity
/// and semantic mass alongside imports, so "has an edge" against it answers
/// a different, broader question than the one the measurement in
/// [`MAINLAND_SHARE_PERCENT`]'s doc comment used -- checked against it
/// directly, `layout.graph` reclassifies a large share of dify's measured
/// unconnected files as islands (a file with no import can still carry
/// cochange/proximity/semantic mass) and reproduces neither dify's nor
/// crawlab's island/unconnected split, while `imports` reproduced dify's
/// four numbers (27/79/110/465, finding 14's corpus, before finding 15)
/// exactly. `imports` is also the edge set the
/// viewer already renders (`E`), so "unconnected" ends up meaning what a
/// person looking at the map would call it: no drawn edge to anything.
///
/// This reads the partition; it does not change it -- membership is never
/// touched, and nothing here calls the partitioner. `FileId` is the node
/// index because extraction emits nodes in the same lexical order used to
/// assign ids.
///
/// Iterates `BTreeMap`s throughout, never a `HashMap`/`HashSet` -- finding 9:
/// a set's iteration order leaking into a seeded stage is exactly how the
/// reference's geometry stopped being reproducible.
fn classify_districts(
    membership: &[usize],
    imports: &[(FileId, FileId, f64)],
) -> BTreeMap<usize, DistrictClass> {
    let total = membership.len();
    let mut connected = vec![false; membership.len()];
    for &(a, b, _) in imports {
        let a = a as usize;
        let b = b as usize;
        if a < connected.len() && b < connected.len() {
            connected[a] = true;
            connected[b] = true;
        }
    }
    let mut sizes = BTreeMap::<usize, usize>::new();
    let mut has_import_edge = BTreeMap::<usize, bool>::new();
    for (file, &district) in membership.iter().enumerate() {
        *sizes.entry(district).or_default() += 1;
        let entry = has_import_edge.entry(district).or_insert(false);
        *entry = *entry || connected[file];
    }
    sizes
        .into_iter()
        .map(|(district, size)| {
            let class = if size * 100 >= total.max(1) * MAINLAND_SHARE_PERCENT {
                DistrictClass::Mainland
            } else if has_import_edge[&district] {
                DistrictClass::Island
            } else {
                DistrictClass::Unconnected
            };
            (district, class)
        })
        .collect()
}

/// Upper bound on how far `blobs::place()` can scatter a district's own
/// files from its centroid. That function normalises each district's
/// internal spring layout to a radius of at most `cap = 1.5 *
/// <90th-percentile radius>` (after mean-centering, unconditionally, no
/// matter how the points inside got there) before multiplying by `scale =
/// 1.30 * sqrt(size / total)`, so no file in a `size`-file district (out of
/// `total`) ever lands further than `1.5 * 1.30 ~= 1.95` times that scale
/// from the district's own centroid. Doubled here as margin over that
/// already-generous bound: coordinates are not gated (`parity.rs`, finding
/// 11), so overshooting costs nothing but a little empty map, while
/// undershooting risks the one thing that *is* load-bearing -- see
/// [`relocate_offshore`].
const REACH_SAFETY_FACTOR: f64 = 3.9;

/// See [`REACH_SAFETY_FACTOR`].
fn reach(size: usize, total: usize) -> f64 {
    REACH_SAFETY_FACTOR * (size as f64 / total.max(1) as f64).sqrt()
}

/// Extra flat buffer added on top of [`reach`] between two rings, so an
/// estimate that is merely close (rather than exact) still leaves room.
const RING_MARGIN: f64 = 0.5;

/// Moves every island district's centroid onto a ring around mainland's own
/// centre of mass, and every unconnected district's centroid onto a second
/// ring beyond that, each evenly spaced by sorted district id (determinism,
/// finding 9 -- an arbitrary or hash-derived angle assignment would make
/// the offshore layout depend on iteration order that isn't guaranteed
/// stable).
///
/// The ring's centre and radius come from the *mainland* districts'
/// own centroids and sizes, not a fixed point and constant. A fixed
/// `(0.5, 0.5)` centre assumes district centroids land in `[0, 1]^2`
/// centred on that point (`pipeline::normalize_points` does normalise into
/// `[0, 1]^2`, but by fitting the box to *every* district's pre-relocation
/// centroid, mainland and not) -- true when nothing pulls the box
/// off-centre, but a repository with a real sub-1% district already in it
/// (prometheus, measured: three) can do exactly that. A ring built from
/// where mainland actually ended up, and how far its own districts can
/// reach, is safe by construction instead of by coincidence.
///
/// Why the radius has to clear mainland's *reach* and not just its
/// centroids: `blobs::relax()`'s only cross-district effect is pushing
/// apart any two circles that overlap, run once over every district
/// including these two new rings. A first version of this function used a
/// fixed radius sized for a district spread evenly across a unit square;
/// on prometheus, whose largest mainland district holds 30% of its files,
/// that district's own scatter reached past the ring and `relax` pushed a
/// mainland file that should not have moved by 0.135. Sizing the ring from
/// every mainland district's actual centroid and [`reach`] closes *that*
/// gap: this function only ever mutates island/unconnected entries, so a
/// mainland district's centroid, and everything `blobs::place()` derives
/// from it, is untouched byte for byte, and the ring construction now
/// guarantees no *new* mainland/offshore overlap for `relax()` to act on.
///
/// It does not close every gap on every repository, and prometheus is the
/// proof: with the ring fixed, its largest mainland district still moves
/// by up to 0.135 (all nine acceptance fixtures otherwise show zero
/// movement -- verified, not assumed). The cause is structural, not a
/// sizing miss. Prometheus already has three sub-1% districts, and
/// `pipeline::normalize_points` fits its `[0, 1]^2` box to *every*
/// district's force-layout centroid, mainland and not -- an isolated
/// district with no inter-district edges (exactly what an island or an
/// unconnected group is) can land anywhere in that force layout, and on
/// prometheus one lands close enough to compress every mainland centroid
/// into a tight cluster a small fraction of the districts' own `relax`
/// radii apart. `relax`'s *today* already pushes mainland apart from that
/// one nearby small district to resolve the resulting overlap -- moving
/// the small district away (the entire point of this feature) necessarily
/// removes a push that was already part of "today's placement" for this
/// specific repository. No placement rule can both relocate a district and
/// leave undisturbed a mainland district that today's code only reached its
/// current position by pushing against that same district's old one. This
/// is the only one of the nine acceptance fixtures where it applies
/// (docs/ARCHITECTURE.md: prometheus is the only one with any sub-1%
/// district at all), the movement is bounded and small (0.135, under
/// finding 3's real per-commit churn baseline), and it is a `District.blob`
/// /`NodeRow` coordinate either way -- not gated by `parity.rs` (finding
/// 11), which this change leaves at 100% placement / 0.0000 modularity
/// delta on all nine fixtures including prometheus (verified).
///
/// What this does *not* isolate: `blobs::contours()` fits one shared raster
/// to the bounding box of every point on the map, so pushing districts out
/// here coarsens that raster and can nudge a mainland polygon's *outline*
/// by a fraction of a grid cell. `District.blob` is float geometry
/// `parity.rs` already excludes from the acceptance gate (finding 11's
/// ruling) precisely because it is reproduced by a spring layout and a
/// raster, not by the partition -- the coordinates the gate actually
/// covers (membership, and each file's own point) are decided in
/// `blobs::place()`/`blobs::relax()` before `contours()` ever runs, and are
/// unaffected either way.
fn relocate_offshore(
    districts: &mut BTreeMap<usize, LayoutDistrict>,
    classes: &BTreeMap<usize, DistrictClass>,
) {
    let total = districts
        .values()
        .map(|district| district.size)
        .sum::<usize>();
    let mainland = classes
        .iter()
        .filter(|(_, &class)| class == DistrictClass::Mainland)
        .map(|(&district, _)| district)
        .collect::<Vec<_>>();

    // Mainland's own centre of mass (size-weighted, so one huge district
    // does not get out-voted by several tiny ones) and how far past it its
    // own districts can reach -- see this function's doc comment for why
    // neither can be assumed fixed.
    let center = if mainland.is_empty() {
        RING_CENTER_FALLBACK
    } else {
        let (mut x, mut y, mut weight) = (0.0, 0.0, 0.0);
        for &district in &mainland {
            let entry = &districts[&district];
            let size = entry.size as f64;
            x += entry.centroid[0] * size;
            y += entry.centroid[1] * size;
            weight += size;
        }
        [x / weight.max(1.0), y / weight.max(1.0)]
    };
    let mainland_clearance = mainland
        .iter()
        .map(|district| {
            let entry = &districts[district];
            let distance = ((entry.centroid[0] - center[0]).powi(2)
                + (entry.centroid[1] - center[1]).powi(2))
            .sqrt();
            distance + reach(entry.size, total)
        })
        .fold(0.0_f64, f64::max);

    let island_reach = classes
        .iter()
        .filter(|(_, &class)| class == DistrictClass::Island)
        .map(|(district, _)| reach(districts[district].size, total))
        .fold(0.0_f64, f64::max);
    let island_radius = mainland_clearance + RING_MARGIN;
    place_ring(
        districts,
        classes,
        DistrictClass::Island,
        center,
        island_radius,
    );

    // Beyond the island ring's own outer edge (its radius plus the widest
    // island's reach), not just beyond mainland -- unconnected must clear
    // islands too, or the two offshore rings could overlap each other.
    let unconnected_reach = classes
        .iter()
        .filter(|(_, &class)| class == DistrictClass::Unconnected)
        .map(|(district, _)| reach(districts[district].size, total))
        .fold(0.0_f64, f64::max);
    let unconnected_radius = island_radius + island_reach + RING_MARGIN + unconnected_reach;
    place_ring(
        districts,
        classes,
        DistrictClass::Unconnected,
        center,
        unconnected_radius,
    );
}

/// Only reached when a repository has no mainland district at all -- every
/// file falls in a district holding less than 1% of the total, which needs
/// at least 100 districts of roughly even size to happen, so this is a
/// degenerate-input fallback rather than a value any real corpus measured
/// (crawlab/dify/n8n, docs/FINDINGS.md) is expected to reach.
const RING_CENTER_FALLBACK: [f64; 2] = [0.5, 0.5];

fn place_ring(
    districts: &mut BTreeMap<usize, LayoutDistrict>,
    classes: &BTreeMap<usize, DistrictClass>,
    class: DistrictClass,
    center: [f64; 2],
    radius: f64,
) {
    let ring = classes
        .iter()
        .filter(|(_, &member_class)| member_class == class)
        .map(|(&district, _)| district)
        .collect::<Vec<_>>();
    let count = ring.len();
    for (index, district) in ring.into_iter().enumerate() {
        let angle = std::f64::consts::TAU * index as f64 / count as f64;
        if let Some(entry) = districts.get_mut(&district) {
            entry.centroid = [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ];
        }
    }
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
    let mut layout = pipeline::run_with_variant(
        graph,
        resolution,
        &partitioner,
        initial.as_deref(),
        features.prune_variant,
    )?;
    // Classification and offshore placement (issue #34) read the partition
    // `pipeline::run` just produced -- they never feed back into it. Doing
    // this before naming/geometry rather than after keeps every downstream
    // step (naming, `blobs::build_geometry`) working against the final
    // island/unconnected centroids, so there is exactly one geometry pass,
    // not a normal one followed by a patch-up.
    let classes = classify_districts(&layout.membership, &layout.weighted.imports);
    relocate_offshore(&mut layout.districts, &classes);
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
    let mut geometry = blobs::build_geometry(&layout, &partitioner, features.terrain)?;
    // Unconnected districts get no region: they are not places (issue #34).
    // Their files already have a defined, deterministic point from the
    // scatter above (`compact` still emits a `NodeRow` for every file
    // regardless of class, as the schema requires) -- this only removes the
    // polygon `blobs::contours` drew around them, so the viewer lists
    // rather than draws them.
    for (&district, class) in &classes {
        if *class == DistrictClass::Unconnected {
            geometry.blobs.remove(&district);
        }
    }
    let mut document = compact(
        map_name,
        layout,
        geometry,
        names,
        previous_document,
        &classes,
    );
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
    classes: &BTreeMap<usize, DistrictClass>,
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
        .map(|(a, b, _)| [*a as usize, *b as usize])
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
                    // Every district id `classify_districts` saw came out of
                    // this same `layout.membership`, so this lookup cannot
                    // miss -- indexing rather than a fallible `get` makes
                    // that invariant visible instead of silently defaulting
                    // to `Mainland` if it were ever violated.
                    class: classes[&district],
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
    let mut outgoing = BTreeMap::<(usize, String), BTreeSet<usize>>::new();
    for (source, target, name) in &layout.weighted.uses {
        outgoing
            .entry((*source as usize, name.clone()))
            .or_default()
            .insert(*target as usize);
    }
    let mut cache = BTreeMap::<(usize, String), Option<(usize, usize)>>::new();
    let mut result = BTreeMap::<String, BTreeSet<usize>>::new();
    for (source, target, name) in &layout.weighted.uses {
        let source = *source as usize;
        let target = *target as usize;
        let key = (target, name.clone());
        let definition = cache
            .entry(key)
            .or_insert_with(|| define_site(target, name, &symbol_index, &outgoing, 4));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// District 0 is a lone file (index 0); every other file is one large
    /// district (1, always mainland regardless of the boundary under test),
    /// so `total` reflects the repo size the percentage is taken against.
    fn membership_with_singleton(total: usize) -> Vec<usize> {
        let mut membership = vec![1; total];
        membership[0] = 0;
        membership
    }

    #[test]
    fn a_district_exactly_at_one_percent_is_mainland() {
        // 1 file out of 100 is exactly MAINLAND_SHARE_PERCENT -- the
        // threshold is `>=`, so this must land on mainland, not island.
        let membership = membership_with_singleton(100);
        let classes = classify_districts(&membership, &[]);
        assert_eq!(classes[&0], DistrictClass::Mainland);
    }

    #[test]
    fn a_one_file_district_with_a_kept_edge_below_the_threshold_is_an_island() {
        // 1 file out of 200 is below 1%, and it is incident to a resolved
        // import edge (to a file in the mainland district) -- island, not
        // unconnected.
        let membership = membership_with_singleton(200);
        let imports = vec![(0, 1, 1.0)];
        let classes = classify_districts(&membership, &imports);
        assert_eq!(classes[&0], DistrictClass::Island);
    }

    #[test]
    fn a_one_file_district_with_no_kept_edge_below_the_threshold_is_unconnected() {
        // Same below-threshold share as the island case above, but with no
        // import edge anywhere -- the only difference between the two
        // outcomes classify_districts can produce below the mainland floor.
        let membership = membership_with_singleton(200);
        let classes = classify_districts(&membership, &[]);
        assert_eq!(classes[&0], DistrictClass::Unconnected);
    }

    #[test]
    fn relocate_offshore_leaves_mainland_untouched_and_rings_the_rest() {
        let mut districts = BTreeMap::new();
        districts.insert(
            0,
            LayoutDistrict {
                centroid: [0.3, 0.4],
                area: 0.5,
                rect: [0.0; 4],
                size: 50,
            },
        );
        districts.insert(
            1,
            LayoutDistrict {
                centroid: [0.1, 0.1],
                area: 0.01,
                rect: [0.0; 4],
                size: 2,
            },
        );
        districts.insert(
            2,
            LayoutDistrict {
                centroid: [0.9, 0.9],
                area: 0.005,
                rect: [0.0; 4],
                size: 1,
            },
        );
        let classes = BTreeMap::from([
            (0, DistrictClass::Mainland),
            (1, DistrictClass::Island),
            (2, DistrictClass::Unconnected),
        ]);
        relocate_offshore(&mut districts, &classes);

        // Mainland is byte-for-byte untouched.
        assert_eq!(districts[&0].centroid, [0.3, 0.4]);

        // One mainland district, so it is trivially its own size-weighted
        // centre; the expected ring radii follow the same formula
        // relocate_offshore uses, recomputed independently here so the test
        // checks the *invariant* (clears mainland's reach, then the
        // island's) rather than hardcoding REACH_SAFETY_FACTOR's value.
        let total = 53; // 50 + 2 + 1
        let center = [0.3, 0.4];
        let distance = |centroid: [f64; 2]| {
            ((centroid[0] - center[0]).powi(2) + (centroid[1] - center[1]).powi(2)).sqrt()
        };
        let expected_island_radius = reach(50, total) + RING_MARGIN;
        let island_reach = reach(2, total);
        let expected_unconnected_radius =
            expected_island_radius + island_reach + RING_MARGIN + reach(1, total);

        assert!((distance(districts[&1].centroid) - expected_island_radius).abs() < 1e-9);
        assert!((distance(districts[&2].centroid) - expected_unconnected_radius).abs() < 1e-9);
        // The two offshore rings must not overlap each other either.
        assert!(expected_unconnected_radius > expected_island_radius + island_reach);
    }
}
