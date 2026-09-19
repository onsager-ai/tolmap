use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

const STOP: &[&str] = &[
    "src", "lib", "pkg", "internal", "packages", "core", "app", "python",
];

/// One entry of the naming cache `naming.py::save_cache` writes to
/// `<out>/<name>.names.json` and `name_districts` reads back. Keyed by
/// [`fingerprint`] of the district's (sorted) member list, not by district
/// id -- ids are an artefact of one clustering run and are not stable
/// across a reclustering (finding 4), but "which fingerprint" survives
/// district renumbering, member reordering and even a rerun's IDF namer
/// being skipped entirely on a cache hit. `district`/`size` are carried
/// along only for `eval/seed_names.py`-style tooling that wants to relate a
/// cache entry back to a specific fixture map; naming itself never reads
/// them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub name: String,
    pub district: usize,
    pub size: usize,
}

pub type NameCache = BTreeMap<String, CacheEntry>;

pub fn fingerprint(members: &[String]) -> String {
    let mut members = members.to_vec();
    members.sort();
    let digest = Sha1::digest(members.join("\n").as_bytes());
    format!("{digest:x}")[..12].to_owned()
}

/// `naming.py::load_cache`: a missing or unparsable cache is not an error --
/// a first build for a repository has no cache yet, and the tool must still
/// run (deterministic IDF fallback) rather than fail on it.
pub fn load_cache(path: &Path) -> NameCache {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// `naming.py::save_cache`. Formatting is not required to byte-match the
/// Python writer (`indent=1, sort_keys=True`) -- nothing reads this file
/// except `load_cache`/`eval/seed_names.py`, both of which only need valid
/// JSON with this shape, and a `BTreeMap` already serialises with sorted
/// keys.
pub fn save_cache(path: &Path, cache: &NameCache) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(cache)?;
    fs::write(path, json)
}

/// Deterministic-fallback-only naming, with no cache: every district gets an
/// IDF name computed fresh from its own membership. Exists for callers (and
/// the sole remaining direct test) that want the raw namer without the
/// cache-read/write side effects -- `name_districts` below is what `tolmap
/// build` actually calls, and differs only in consulting a cache first.
#[cfg(test)]
pub fn names_for_membership(files: &[String], membership: &[usize]) -> BTreeMap<String, String> {
    name_districts(files, membership, None).0
}

/// `naming.py::name_districts`, minus the model hook (`naming_prompt`'s
/// contract is unimplemented on the Rust side -- HANDOFF.md lists wiring a
/// naming model as scaffolding, not milestone-1 scope). Returns the
/// district-id -> name map for this build, and the cache as it should be
/// written back (existing hits carried through unchanged, new fallback
/// names added) -- the caller decides whether/where to persist it.
///
/// Never renames a district whose membership fingerprint is already in the
/// cache (CLAUDE.md: "never rename a district without the previous name in
/// hand"). A cache miss -- membership genuinely changed, or there is no
/// cache at all -- falls through to the same IDF namer `names_for_membership`
/// always used.
pub fn name_districts(
    files: &[String],
    membership: &[usize],
    cache_path: Option<&Path>,
) -> (BTreeMap<String, String>, NameCache) {
    let mut groups = BTreeMap::<usize, Vec<String>>::new();
    let mut encounter = Vec::new();
    for (file, &district) in files.iter().zip(membership) {
        if !groups.contains_key(&district) {
            encounter.push(district);
        }
        groups.entry(district).or_default().push(file.clone());
    }
    let mut all_files = Vec::new();
    for district in &encounter {
        all_files.extend(groups[district].iter().cloned());
    }
    let (document_frequency, total) = segment_df(&all_files);

    let mut cache = cache_path.map(load_cache).unwrap_or_default();

    let encounter_position = encounter
        .iter()
        .enumerate()
        .map(|(index, &district)| (district, index))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = encounter.clone();
    ordered.sort_by_key(|district| {
        (
            std::cmp::Reverse(groups[district].len()),
            encounter_position[district],
        )
    });

    let mut used = BTreeSet::<String>::new();
    let mut output = BTreeMap::new();
    for district in ordered {
        let members = &groups[&district];
        let key = fingerprint(members);
        let name = match cache.get(&key) {
            Some(hit) => hit.name.clone(),
            None => {
                let mut name = auto_name(members, &document_frequency, total);
                name = name.trim().chars().take(32).collect();
                if used.contains(&name) {
                    let base = name.clone();
                    let number = used.iter().filter(|value| value.starts_with(&base)).count() + 1;
                    name = format!("{base} {number}");
                }
                cache.insert(
                    key,
                    CacheEntry {
                        name: name.clone(),
                        district,
                        size: members.len(),
                    },
                );
                name
            }
        };
        used.insert(name.clone());
        output.insert(district.to_string(), name);
    }
    (output, cache)
}

fn segment_df(files: &[String]) -> (BTreeMap<String, usize>, usize) {
    let mut frequency = BTreeMap::new();
    for file in files {
        let segments = file.split('/').rev().skip(1).collect::<BTreeSet<_>>();
        for segment in segments {
            *frequency.entry(segment.to_owned()).or_default() += 1;
        }
    }
    (frequency, files.len())
}

fn auto_name(
    members: &[String],
    document_frequency: &BTreeMap<String, usize>,
    total: usize,
) -> String {
    let mut score = BTreeMap::<String, (f64, usize)>::new();
    let mut order = 0;
    for file in members {
        let parts = file.split('/').collect::<Vec<_>>();
        for (depth, part) in parts[..parts.len().saturating_sub(1)]
            .iter()
            .filter(|part| !STOP.contains(part))
            .enumerate()
        {
            let frequency = document_frequency.get(*part).copied().unwrap_or(0);
            if total > 0 && frequency as f64 > total as f64 * 0.6 {
                continue;
            }
            let idf = if total > 0 {
                ((total + 1) as f64 / (frequency + 1) as f64).ln()
            } else {
                1.0
            };
            let entry = score.entry((*part).to_owned()).or_insert_with(|| {
                let current = order;
                order += 1;
                (0.0, current)
            });
            entry.0 += 1.0 / (1.0 + depth as f64 * 0.4) * idf;
        }
    }
    let mut ranked = score
        .into_iter()
        .filter(|(_, (value, _))| *value > 0.08)
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
             .0
            .partial_cmp(&left.1 .0)
            .unwrap_or(Ordering::Equal)
            .then(left.1 .1.cmp(&right.1 .1))
    });
    if let Some((first, (first_score, _))) = ranked.first() {
        if let Some((second, (second_score, _))) = ranked.get(1) {
            if second_score > &(first_score * 0.55) {
                return format!("{first} & {second}");
            }
        }
        return first.clone();
    }
    filename_name(members)
}

fn filename_name(members: &[String]) -> String {
    let mut stems = members
        .iter()
        .filter_map(|file| {
            let stem = file
                .rsplit('/')
                .next()
                .unwrap_or(file)
                .rsplit_once('.')
                .map_or_else(|| file.as_str(), |(stem, _)| stem)
                .trim_start_matches('_');
            (!stem.is_empty() && !matches!(stem, "init" | "main" | "index" | "base" | "common"))
                .then(|| stem.to_owned())
        })
        .collect::<Vec<_>>();
    if stems.is_empty() {
        return "misc".to_owned();
    }
    let mut words = BTreeMap::<String, (usize, usize)>::new();
    let mut order = 0;
    for stem in &stems {
        for word in stem.replace('-', "_").split('_') {
            if word.len() <= 3 {
                continue;
            }
            let entry = words.entry(word.to_owned()).or_insert_with(|| {
                let current = order;
                order += 1;
                (0, current)
            });
            entry.0 += 1;
        }
    }
    let mut common = words
        .into_iter()
        .filter(|(_, (count, _))| *count as f64 >= 2.0_f64.max(stems.len() as f64 * 0.3))
        .collect::<Vec<_>>();
    common.sort_by_key(|(_, (count, order))| (std::cmp::Reverse(*count), *order));
    if !common.is_empty() {
        return common
            .into_iter()
            .take(2)
            .map(|(word, _)| word)
            .collect::<Vec<_>>()
            .join(" & ");
    }
    stems.sort_by_key(|stem| (stem.len(), stem.clone()));
    stems.into_iter().take(2).collect::<Vec<_>>().join(" & ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_root_cannot_name_every_district() {
        let files = vec![
            "src/pkg/http/client.py".to_owned(),
            "src/pkg/http/server.py".to_owned(),
            "src/pkg/db/query.py".to_owned(),
            "src/pkg/db/model.py".to_owned(),
        ];
        let names = names_for_membership(&files, &[0, 0, 1, 1]);
        assert_eq!(names["0"], "http");
        assert_eq!(names["1"], "db");
    }
}
