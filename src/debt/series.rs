//! Per-commit structural debt measured in one base partition (issue #372).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{findings_pinned_with_renames, map_sha256, DebtReport, DistrictCycle};
use crate::check::{self, CheckError};
use crate::schema::MapDocument;

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DebtSeries {
    pub version: u32,
    pub base: String,
    pub head: String,
    pub first_parent: bool,
    pub lower_bound: bool,
    pub points: Vec<DebtSeriesPoint>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct DebtSeriesPoint {
    pub commit: String,
    pub cycles: usize,
    pub misplaced: usize,
    pub god_files: usize,
    pub hotspots: usize,
    pub hidden_coupling: usize,
    pub unreached: usize,
    pub placed_new_files: usize,
    pub unplaced_new_files: usize,
    pub anchored_share: Option<f64>,
    pub added: Vec<DebtFindingIdentity>,
    pub removed: Vec<DebtFindingIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct DebtFindingIdentity {
    pub kind: DebtFindingKind,
    pub key: String,
}

// Declaration order matches the lexical order of the serialized kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DebtFindingKind {
    Cycle,
    God,
    HiddenCoupling,
    Hotspot,
    Misplaced,
    Unreached,
}

impl DebtFindingKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::God => "god",
            Self::HiddenCoupling => "hidden_coupling",
            Self::Hotspot => "hotspot",
            Self::Misplaced => "misplaced",
            Self::Unreached => "unreached",
        }
    }
}

fn cycle_key(cycle: &DistrictCycle) -> String {
    let mut names = cycle
        .route
        .iter()
        .map(|id| {
            cycle
                .districts
                .iter()
                .find(|district| district.id == *id)
                .expect("cycle route stays inside its districts")
                .name
                .as_str()
        })
        .collect::<Vec<_>>();
    let rotation = |start: usize| names[start..].iter().chain(&names[..start]);
    let start = (0..names.len())
        .min_by(|&a, &b| rotation(a).cmp(rotation(b)))
        .expect("district cycle has a route");
    names.rotate_left(start);
    names.join(" -> ")
}

fn finding_identities(report: &DebtReport) -> BTreeSet<DebtFindingIdentity> {
    use DebtFindingKind::{Cycle, God, HiddenCoupling, Hotspot, Misplaced, Unreached};

    let identity = |kind, key| DebtFindingIdentity { kind, key };
    report
        .cycles
        .iter()
        .map(|cycle| identity(Cycle, cycle_key(cycle)))
        .chain(
            report
                .god_files
                .iter()
                .map(|f| identity(God, f.path.clone())),
        )
        .chain(report.hidden_coupling.iter().map(|pair| {
            let mut paths = [pair.left.as_str(), pair.right.as_str()];
            paths.sort_unstable();
            identity(HiddenCoupling, paths.join(" <-> "))
        }))
        .chain(
            report
                .hotspots
                .iter()
                .map(|f| identity(Hotspot, f.path.clone())),
        )
        .chain(
            report
                .misplaced
                .iter()
                .map(|f| identity(Misplaced, f.path.clone())),
        )
        .chain(
            report
                .unreached
                .iter()
                .map(|f| identity(Unreached, f.path.clone())),
        )
        .collect()
}

fn commits(
    repo: &Path,
    base: &str,
    head: &str,
    first_parent: bool,
) -> Result<Vec<String>, CheckError> {
    let range = format!("{base}..{head}");
    let mut args = vec!["rev-list", "--topo-order", "--reverse"];
    if first_parent {
        args.push("--first-parent");
    }
    args.extend([range.as_str(), "--"]);
    let listed = check::git_stdout(repo, &args)?;
    Ok(std::iter::once(base.to_owned())
        .chain(String::from_utf8_lossy(&listed).lines().map(str::to_owned))
        .collect())
}

fn cold_map(repo: &Path, commit: &str) -> Result<(MapDocument, Vec<u8>), CheckError> {
    let raw = check::build_cold_map(repo, commit).map_err(|error| match error {
        CheckError::Input(message) => CheckError::Input(format!("{commit}: {message}")),
        CheckError::Internal(error) => {
            CheckError::Internal(error.context(format!("{commit}: build cold map")))
        }
    })?;
    let map = serde_json::from_slice(&raw).map_err(|error| {
        CheckError::Internal(anyhow::anyhow!("{commit}: parse cold map: {error}"))
    })?;
    Ok((map, raw))
}

fn pinned_report(
    repo: &Path,
    base: &str,
    commit: &str,
    base_map: &MapDocument,
    map: &MapDocument,
    base_sha256: &str,
) -> Result<(MapDocument, DebtReport), CheckError> {
    let changes = check::diff(repo, base, Some(commit))?;
    Ok(findings_pinned_with_renames(
        map,
        base_map,
        base_sha256.to_owned(),
        Some(base.to_owned()),
        &check::renamed_from(&changes),
    ))
}

fn parse_range(range: &str) -> Result<(&str, &str), CheckError> {
    let malformed = || CheckError::Input("--range must be two git refs in A..B form".to_owned());
    let (base, head) = range.split_once("..").ok_or_else(malformed)?;
    if base.is_empty()
        || head.is_empty()
        || range.contains("...")
        || base.contains("..")
        || head.contains("..")
    {
        return Err(malformed());
    }
    Ok((base, head))
}

fn build(repo: &Path, range: &str, first_parent: bool) -> Result<DebtSeries, CheckError> {
    let (base, head) = parse_range(range)?;
    let repo = check::repository_root(repo, true)?;
    let base = check::resolve_commit(&repo, base, "--range start")?;
    let head = check::resolve_commit(&repo, head, "--range end")?;
    let commits = commits(&repo, &base, &head, first_parent)?;
    let (base_map, raw) = cold_map(&repo, &base)?;
    let base_sha256 = map_sha256(&raw);
    drop(raw);
    let mut points = Vec::with_capacity(commits.len());
    let mut previous = BTreeSet::new();
    for commit in commits {
        let head_map;
        let map = if commit == base {
            &base_map
        } else {
            head_map = cold_map(&repo, &commit)?.0;
            &head_map
        };
        let (_, report) = pinned_report(&repo, &base, &commit, &base_map, map, &base_sha256)?;
        let current = finding_identities(&report);
        let pin = report.pinned.expect("pinned report records placement");
        let anchored = report.files - pin.placed_new_files - pin.unplaced_new_files;
        points.push(DebtSeriesPoint {
            commit,
            cycles: report.cycles.len(),
            misplaced: report.misplaced.len(),
            god_files: report.god_files.len(),
            hotspots: report.hotspots.len(),
            hidden_coupling: report.hidden_coupling.len(),
            unreached: report.unreached.len(),
            placed_new_files: pin.placed_new_files,
            unplaced_new_files: pin.unplaced_new_files,
            anchored_share: (report.files != 0).then(|| anchored as f64 / report.files as f64),
            added: current.difference(&previous).cloned().collect(),
            removed: previous.difference(&current).cloned().collect(),
        });
        previous = current;
    }
    Ok(DebtSeries {
        version: 1,
        base,
        head,
        first_parent,
        lower_bound: true,
        points,
    })
}

pub fn render(
    repo: &Path,
    range: &str,
    first_parent: bool,
    format: &str,
) -> Result<String, CheckError> {
    let series = build(repo, range, first_parent)?;
    if format == "json" {
        let json = serde_json::to_string_pretty(&series)
            .map_err(|error| CheckError::Internal(error.into()))?;
        return Ok(format!("{json}\n"));
    }
    let mut text = format!(
        "Debt series anchored to {} (counts are lower bounds)\n",
        series.base
    );
    text.push_str("commit\tcycles\tmisplaced\tgod_files\thotspots\thidden_coupling\tunreached\tplaced_new_files\tunplaced_new_files\tanchored_share\n");
    for point in series.points {
        let share = point
            .anchored_share
            .map_or_else(|| "-".to_owned(), |share| format!("{:.2}%", share * 100.0));
        writeln!(
            text,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{share}",
            point.commit,
            point.cycles,
            point.misplaced,
            point.god_files,
            point.hotspots,
            point.hidden_coupling,
            point.unreached,
            point.placed_new_files,
            point.unplaced_new_files
        )
        .unwrap();
        for (sign, findings) in [('+', point.added), ('-', point.removed)] {
            for finding in findings {
                writeln!(text, "  {sign} {} {}", finding.kind.as_str(), finding.key).unwrap();
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn range_parser_rejects_malformed_ranges_before_git_resolution() {
        let error = "--range must be two git refs in A..B form";
        for range in ["..HEAD", "HEAD..", "HEAD...main", "A..B..C"] {
            assert!(
                matches!(parse_range(range), Err(CheckError::Input(message)) if message == error)
            );
        }
        assert_eq!(parse_range("A..B").unwrap(), ("A", "B"));
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit(repo: &Path, message: &str) -> String {
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", message]);
        git(repo, &["rev-parse", "HEAD"])
    }

    fn fixture() -> (tempfile::TempDir, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-qb", "trunk"]);
        git(repo, &["config", "user.name", "Fixture"]);
        git(repo, &["config", "user.email", "fixture@example.com"]);
        fs::write(repo.join("pyproject.toml"), "[project]\nname = 'fixture'\n").unwrap();
        fs::create_dir(repo.join("fixture")).unwrap();
        for (file, contents) in [
            ("__init__.py", ""),
            ("anchor.py", "from .core import item\n\nanswer = item + 1\n"),
            ("core.py", "item = 42\n"),
            ("old.py", "from .core import item\n\nother = item + 2\n"),
        ] {
            fs::write(repo.join("fixture").join(file), contents).unwrap();
        }
        let base = commit(repo, "base");
        fs::write(repo.join("README.md"), "a docs-only change\n").unwrap();
        let docs = commit(repo, "docs");
        git(repo, &["mv", "fixture/old.py", "fixture/renamed.py"]);
        let renamed = commit(repo, "rename");
        (dir, base, docs, renamed)
    }

    #[test]
    fn cold_series_is_repeatable_and_keeps_renames_anchored() {
        let (dir, base, docs, renamed) = fixture();
        let repo = dir.path();
        let range = format!("{base}..{renamed}");
        let json = render(repo, &range, false, "json").unwrap();
        assert_eq!(
            json,
            render(&repo.join("fixture"), &range, false, "json").unwrap()
        );
        let series: DebtSeries = serde_json::from_str(&json).unwrap();
        let [baseline, unchanged, renamed_point] = &series.points[..] else {
            panic!("expected three points");
        };
        let identity = |path: &str| DebtFindingIdentity {
            kind: DebtFindingKind::Unreached,
            key: path.to_owned(),
        };
        assert!(baseline.added.contains(&identity("fixture/old.py")));
        assert!(!baseline.added.contains(&identity("fixture/renamed.py")));
        assert!(baseline.removed.is_empty());
        assert!(unchanged.added.is_empty());
        assert!(unchanged.removed.is_empty());
        assert_eq!(baseline.unreached, renamed_point.unreached);
        assert!(renamed_point
            .added
            .contains(&identity("fixture/renamed.py")));
        assert!(renamed_point.removed.contains(&identity("fixture/old.py")));
        for point in &series.points {
            assert!(point.added.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(point.removed.windows(2).all(|pair| pair[0] < pair[1]));
        }
        let baseline_count = baseline.cycles
            + baseline.misplaced
            + baseline.god_files
            + baseline.hotspots
            + baseline.hidden_coupling
            + baseline.unreached;
        assert_eq!(baseline.added.len(), baseline_count);

        let text = render(repo, &range, false, "text").unwrap();
        let renamed_text = text.split_once(&format!("\n{renamed}\t")).unwrap().1;
        assert!(renamed_text.contains("\n  + unreached fixture/renamed.py\n"));
        assert!(renamed_text.contains("\n  - unreached fixture/old.py\n"));
        for (point, commit) in series.points.iter().zip([&base, &docs, &renamed]) {
            assert_eq!(&point.commit, commit);
            assert_eq!(point.anchored_share, Some(1.0));
            assert_eq!((point.placed_new_files, point.unplaced_new_files), (0, 0));
        }
        assert_eq!(
            git(repo, &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1
        );
    }

    #[test]
    fn pinned_misplaced_findings_survive_repartition_and_rename() {
        let (dir, base, docs, renamed) = fixture();
        // Give both files enough edges into another district to make them
        // misplaced. The independently partitioned head maps hide them.
        let map: MapDocument = serde_json::from_value(serde_json::json!({
            "repo": "fixture", "q": 0.0, "names": {"0": "source", "1": "target"}, "districts": {},
            "F": ["fixture/anchor.py", "fixture/old.py", "a.py", "b.py", "c.py", "d.py", "e.py"],
            "N": (0..7).map(|i| serde_json::json!([
                if i >= 2 { 1 } else { 0 }, 0.0, 0.0, 10, 1, 1, 0.0, 0.0, 0.0, 0.0, 0.0
            ])).collect::<Vec<_>>(),
            "E": (0..2).flat_map(|i| (2..7).map(move |j| [i, j])).collect::<Vec<_>>(),
            "L": [], "S": {}, "U": {}, "roads": [], "lang": "python",
        }))
        .unwrap();
        for commit in commits(dir.path(), &base, &renamed, false).unwrap() {
            let mut head = map.clone();
            let path = if commit == renamed {
                "fixture/renamed.py"
            } else {
                "fixture/old.py"
            };
            head.files[1] = path.to_owned();
            if commit == docs || commit == renamed {
                head.nodes[0].0 .0 = 1;
                head.nodes[1].0 .0 = 1;
                assert!(super::super::findings(&head).misplaced.is_empty());
            }
            let (frame, report) =
                pinned_report(dir.path(), &base, &commit, &map, &head, "sha").unwrap();
            assert_eq!(frame.nodes[0].district(), 0);
            assert_eq!(frame.nodes[1].district(), 0);
            let mut misplaced = report
                .misplaced
                .iter()
                .map(|finding| finding.path.as_str())
                .collect::<Vec<_>>();
            misplaced.sort_unstable();
            assert_eq!(misplaced, ["fixture/anchor.py", path]);
        }
    }

    #[test]
    fn cycle_identity_uses_the_smallest_rotation_of_the_directed_route() {
        let map: MapDocument = serde_json::from_value(serde_json::json!({
            "repo": "fixture", "q": 0.0, "names": {"0": "beta", "1": "alpha", "2": "gamma"},
            "districts": {}, "F": ["a.py", "b.py", "c.py"],
            "N": (0..3).map(|i| serde_json::json!([
                i, 0.0, 0.0, 10, 1, 1, 0.0, 0.0, 0.0, 0.0, 0.0
            ])).collect::<Vec<_>>(),
            "E": [[0, 1], [1, 2], [2, 0]], "L": [], "S": {}, "U": {}, "roads": [],
            "lang": "python",
        }))
        .unwrap();
        let mut report = super::super::findings(&map);
        let expected = DebtFindingIdentity {
            kind: DebtFindingKind::Cycle,
            key: "alpha -> gamma -> beta".to_owned(),
        };
        assert!(finding_identities(&report).contains(&expected));
        report.cycles[0].route.rotate_left(1);
        assert!(finding_identities(&report).contains(&expected));
    }

    #[test]
    fn first_parent_excludes_commits_on_the_merged_branch() {
        let (dir, base, docs, renamed) = fixture();
        let repo = dir.path();
        git(repo, &["switch", "-qc", "side", &docs]);
        fs::write(repo.join("extra.md"), "side branch change\n").unwrap();
        let side = commit(repo, "side change");
        git(repo, &["switch", "-q", "trunk"]);
        git(repo, &["merge", "--no-ff", "-qm", "merge side", "side"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        let all = commits(repo, &base, &head, false).unwrap();
        assert_eq!(all.first(), Some(&base));
        assert_eq!(all.last(), Some(&head));
        assert_eq!(all.len(), 5);
        assert!(all.contains(&side));
        assert_eq!(
            commits(repo, &base, &head, true).unwrap(),
            [base, docs, renamed, head]
        );
    }
}
