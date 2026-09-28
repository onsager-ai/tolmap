//! What the service accepts from a worker's `result` event before it stores
//! a map.
//!
//! The worker runs at its own uid (docs/API.md, "Worker isolation") and
//! parses repositories nobody reviewed, so its result is input to check, not
//! a set of instructions. The service works out every name it acts on
//! itself -- the commit from its own checkout, the file names from the
//! repository name and the job's output directory -- and requires the
//! worker's reported values to equal them.
//!
//! The files are then moved, not checked in place. The worker's uid owns the
//! output directory and everything in it, so a check on
//! `output/<repo>.json` followed by a rename of that path would examine one
//! thing and move another if anything under that uid changed the directory
//! in between. Instead [`adopt`] first renames the whole output directory
//! into a staging directory the service has just created (its own uid, mode
//! `0700`), then each expected entry out of it, and examines each entry only
//! once it sits in a directory nothing but the service can write. `rename`
//! never follows the last component of either path, so a symlink is moved
//! as a symlink and then refused, and the directory it pointed into is
//! never touched.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::naming::NameCache;

/// Bound on the names cache the service reads back. It holds one short entry
/// per district, so a real one is a few kilobytes; anything near this is not
/// a names cache.
const NAMES_CACHE_MAX_BYTES: u64 = 16 << 20;

/// A git object id as `git rev-parse` prints it: 40 lowercase hex digits for
/// SHA-1, 64 for a SHA-256 repository. The commit becomes a stored file name
/// (`<commit>.json`), so nothing else is accepted.
pub(crate) fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Why a worker's result was not stored.
#[derive(Debug)]
pub(crate) enum Refused {
    /// The result is not what a worker for this job produces.
    Invalid(String),
    /// The filesystem failed underneath an otherwise acceptable result.
    Io(std::io::Error),
}

impl From<std::io::Error> for Refused {
    fn from(error: std::io::Error) -> Self {
        Refused::Io(error)
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Invalid(message) => f.write_str(message),
            Refused::Io(error) => write!(f, "{error}"),
        }
    }
}

fn invalid(message: impl Into<String>) -> Refused {
    Refused::Invalid(message.into())
}

/// Just enough of the map document's own shape to check a worker's reported
/// `files`/`districts` counts against what the map it shipped actually
/// contains (docs/WORKER_TIER.md §5.4) -- not the full `MapDocument`
/// (`schema.rs`), so a field added there never has to touch this check, and
/// reading two array/object lengths stays cheap on the corpus's largest
/// maps. `#[serde(deny_unknown_fields)]` is deliberately not set: this type
/// exists to count, not to validate the document's shape.
#[derive(serde::Deserialize)]
struct MapCounts {
    #[serde(rename = "F")]
    files: Vec<serde::de::IgnoredAny>,
    districts: std::collections::BTreeMap<String, serde::de::IgnoredAny>,
}

/// Refuses a result whose reported `files`/`districts` disagree with the
/// map document it just shipped -- the second half of "Checking results"
/// (docs/WORKER_TIER.md §5.4), next to the commit check `adopt`'s caller
/// already made. `path` is the map already moved into staging, so this
/// reads the service's own copy, never anything still under the worker's
/// uid.
fn check_counts(path: &Path, files: usize, districts: usize) -> Result<(), Refused> {
    let bytes = std::fs::read(path)?;
    let counts: MapCounts = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("the map is not a valid map document: {error}")))?;
    if counts.files.len() != files {
        return Err(invalid(format!(
            "the result's files ({files}) does not match the map's F ({})",
            counts.files.len()
        )));
    }
    if counts.districts.len() != districts {
        return Err(invalid(format!(
            "the result's districts ({districts}) does not match the map's districts ({})",
            counts.districts.len()
        )));
    }
    Ok(())
}

/// Checks that `bytes` is a map document, by the same reading
/// [`check_counts`] makes of a result's map: before a map is served early
/// (docs/UX.md §12), nothing else vouches for it yet.
pub(crate) fn check_map_bytes(bytes: &[u8]) -> Result<(), Refused> {
    serde_json::from_slice::<MapCounts>(bytes)
        .map(|_| ())
        .map_err(|error| invalid(format!("the map is not a valid map document: {error}")))
}

/// Bound on a map read back while its worker still runs. The largest corpus
/// maps are tens of megabytes; anything near this is not a map.
const EARLY_MAP_MAX_BYTES: u64 = 1 << 30;

/// Reads the map a job child has just written at `path` -- its output
/// directory's `<repo>.json`, once its `write_map` stage has finished -- so
/// it can open before the symbol stages do (docs/UX.md §12).
///
/// Unlike [`adopt`], the child is still running and still owns the
/// directory, so the file cannot be moved out of its reach first. It is
/// opened instead, never through a symlink at the last component and never
/// blocking on a FIFO, and everything is checked on the open file itself
/// rather than on the path: a regular file with one link, owned by `owner`
/// when given (the worker's uid). Whatever the path named, then, what is
/// read is a file the worker's own uid owns, which is content the worker
/// could have written as its map anyway. The bytes are copied out whole,
/// so a later write by the child changes nothing already read, and they
/// must be a map document ([`check_map_bytes`]).
pub(crate) fn read_early_map(path: &Path, owner: Option<u32>) -> Result<Vec<u8>, Refused> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(invalid("the map is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(invalid("the map has more than one link"));
        }
        if owner.is_some_and(|uid| meta.uid() != uid) {
            return Err(invalid("the map is not owned by the worker"));
        }
    }
    #[cfg(not(unix))]
    let _ = owner;
    let mut bytes = Vec::new();
    file.take(EARLY_MAP_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > EARLY_MAP_MAX_BYTES {
        return Err(invalid("the map is larger than any map tolmap writes"));
    }
    check_map_bytes(&bytes)?;
    Ok(bytes)
}

/// The paths and counts a worker reports in its `result` event. `files` and
/// `districts` are checked against the map document's own `F` and
/// `districts` once it is in staging (docs/WORKER_TIER.md §5.4, "Checking
/// results") -- a result whose counts disagree with the document it shipped
/// is refused rather than stored under numbers nothing produced.
pub(crate) struct Reported<'a> {
    pub map_path: &'a str,
    pub symbols_path: &'a str,
    pub symbols_dir: &'a str,
    pub names_cache: &'a str,
    pub files: usize,
    pub districts: usize,
}

/// A checked result, moved into the staging directory. Every path is inside
/// it, and the service is the only uid that can change what they name.
pub(crate) struct Adopted {
    pub map: PathBuf,
    pub symbols: PathBuf,
    pub symbols_dir: PathBuf,
    pub names: NameCache,
}

/// The entry names a worker writes into its output directory for a map named
/// `repo`: `geometry::build_from_graph_warm_with_progress` writes
/// `<repo>.json` and the `<repo>.names.json` cache,
/// `symbols::write_sibling_with_progress` the siblings it derives from the
/// map path with `with_extension`, derived the same way here so the two
/// cannot disagree.
struct OutputNames {
    map: String,
    symbols: String,
    symbols_dir: String,
    names_cache: String,
}

impl OutputNames {
    fn for_repo(repo: &str) -> Result<Self, Refused> {
        // One plain component each, or the name is not ours to act on.
        fn leaf(path: &Path) -> Option<String> {
            let mut components = path.components();
            match (components.next(), components.next()) {
                (Some(Component::Normal(name)), None) => name.to_str().map(str::to_owned),
                _ => None,
            }
        }
        let map = PathBuf::from(format!("{repo}.json"));
        let names = (|| {
            Some(OutputNames {
                map: leaf(&map)?,
                symbols: leaf(&map.with_extension("symbols.json"))?,
                symbols_dir: leaf(&map.with_extension("symbols"))?,
                names_cache: leaf(Path::new(&format!("{repo}.names.json")))?,
            })
        })();
        names.ok_or_else(|| invalid(format!("repository name {repo:?} is not a file name")))
    }
}

/// Creates `path` as a directory only its owner can enter.
pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Copies `source`, a file in the service's own store, to `dest`, a new file
/// in a job directory the service has just created and not yet handed to the
/// worker (issue #141). The store stays the service's own and `0700`; the
/// worker reads the copy, which `harden_job_dir` then gives to its uid with
/// the rest of the job directory.
///
/// The source is opened by the service, never through a symlink, and must be
/// a regular file. The destination is created, never opened if something is
/// already there -- `create_new` does not follow a symlink at the last
/// component -- and is readable by its owner only.
pub(crate) fn copy_for_worker(source: &Path, dest: &Path) -> std::io::Result<u64> {
    let mut read = std::fs::OpenOptions::new();
    read.read(true);
    let mut write = std::fs::OpenOptions::new();
    write.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        read.custom_flags(libc::O_NOFOLLOW);
        write.mode(0o600);
    }
    let mut from = read.open(source)?;
    if !from.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", source.display()),
        ));
    }
    let mut to = write.open(dest)?;
    std::io::copy(&mut from, &mut to)
}

/// Checks a worker's reported paths against the ones the service expects in
/// `output_dir`, then moves the map, its symbols sibling, the district
/// symbols directory and the names cache into `staging` and checks each
/// there: a regular file (a directory of regular files for the district
/// symbols), one link, owned by `owner` when given (the worker's uid).
///
/// `staging` must be a directory the service has just created with
/// [`create_private_dir`], on the same filesystem as `output_dir`. On
/// failure it may hold part of the result; the caller removes it either way.
pub(crate) fn adopt(
    output_dir: &Path,
    staging: &Path,
    repo: &str,
    reported: &Reported<'_>,
    owner: Option<u32>,
) -> Result<Adopted, Refused> {
    let files = OutputNames::for_repo(repo)?;
    for (field, value, name) in [
        ("map_path", reported.map_path, &files.map),
        ("symbols_path", reported.symbols_path, &files.symbols),
        ("symbols_dir", reported.symbols_dir, &files.symbols_dir),
        ("names_cache", reported.names_cache, &files.names_cache),
    ] {
        if Path::new(value) != output_dir.join(name).as_path() {
            return Err(invalid(format!(
                "the worker's {field} is not the job's own output path"
            )));
        }
    }

    let taken = staging.join("output");
    if !take(output_dir, &taken)? {
        return Err(invalid("the job's output directory is missing"));
    }
    if !is_real_dir(&taken)? {
        return Err(invalid("the job's output directory is not a directory"));
    }

    let map = staging.join("map.json");
    take_file(&taken.join(&files.map), &map, "the map", owner)?;
    check_counts(&map, reported.files, reported.districts)?;
    let symbols = staging.join("symbols.json");
    take_file(
        &taken.join(&files.symbols),
        &symbols,
        "the symbols file",
        owner,
    )?;

    let worker_symbols = staging.join("symbols.worker");
    if !take(&taken.join(&files.symbols_dir), &worker_symbols)? {
        return Err(invalid("the district symbols directory is missing"));
    }
    if !is_real_dir(&worker_symbols)? {
        return Err(invalid("the district symbols directory is not a directory"));
    }
    // A fresh directory of the service's own, so no other uid holds it open
    // and the district files can be checked once they are in it.
    let symbols_dir = staging.join("symbols");
    create_private_dir(&symbols_dir)?;
    let mut entries = std::fs::read_dir(&worker_symbols)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    for entry in entries {
        // `symbols::write_sibling_with_progress` writes `<district>.json`
        // and nothing else; anything more is refused rather than skipped.
        let Some(name) = entry.to_str().filter(|name| is_district_file(name)) else {
            return Err(invalid(format!(
                "unexpected entry {entry:?} in the district symbols directory"
            )));
        };
        take_file(
            &worker_symbols.join(name),
            &symbols_dir.join(name),
            "a district symbols file",
            owner,
        )?;
    }

    // A missing or unreadable cache is an empty one, as `naming::load_cache`
    // treats it; one that is not a plain file of a sane size is refused.
    let names_path = staging.join("names.json");
    let names = if take(&taken.join(&files.names_cache), &names_path)? {
        let meta = check_file(&names_path, "the names cache", owner)?;
        if meta.len() > NAMES_CACHE_MAX_BYTES {
            return Err(invalid("the names cache is too large"));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&names_path)?
            .take(NAMES_CACHE_MAX_BYTES)
            .read_to_end(&mut bytes)?;
        serde_json::from_slice(&bytes).unwrap_or_default()
    } else {
        NameCache::default()
    };

    Ok(Adopted {
        map,
        symbols,
        symbols_dir,
        names,
    })
}

/// Renames `from` to `to`; `Ok(false)` when there is nothing at `from`.
fn take(from: &Path, to: &Path) -> Result<bool, Refused> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn take_file(from: &Path, to: &Path, what: &str, owner: Option<u32>) -> Result<(), Refused> {
    if !take(from, to)? {
        return Err(invalid(format!("{what} is missing")));
    }
    check_file(to, what, owner).map(|_| ())
}

fn is_real_dir(path: &Path) -> std::io::Result<bool> {
    Ok(std::fs::symlink_metadata(path)?.file_type().is_dir())
}

fn is_district_file(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A regular file with one link, owned by `owner` when given. One link: a
/// second name for a file elsewhere is not something the worker wrote.
fn check_file(path: &Path, what: &str, owner: Option<u32>) -> Result<std::fs::Metadata, Refused> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(invalid(format!("{what} is not a regular file")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(invalid(format!("{what} has more than one link")));
        }
        if owner.is_some_and(|uid| meta.uid() != uid) {
            return Err(invalid(format!("{what} is not owned by the worker")));
        }
    }
    #[cfg(not(unix))]
    let _ = owner;
    Ok(meta)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt};

    const REPO: &str = "demo";
    // The fixture map below carries exactly this many files and districts
    // (two district symbol files, ids 0 and 1, are written next to it) --
    // `check_counts` is exercised by overriding `Job::files`/`Job::districts`
    // to disagree with these, not by editing the map itself.
    const FILES: usize = 3;
    const DISTRICTS: usize = 2;

    struct Job {
        dir: tempfile::TempDir,
        output: PathBuf,
        staging: PathBuf,
        owner: u32,
        files: usize,
        districts: usize,
    }

    impl Job {
        /// An output directory as a worker leaves it for `REPO`, and a fresh
        /// staging directory beside a pretend store.
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let output = dir.path().join("work/output");
            std::fs::create_dir_all(output.join(format!("{REPO}.symbols"))).unwrap();
            std::fs::write(
                output.join(format!("{REPO}.json")),
                br#"{"F": ["a.py", "b.py", "c.py"], "districts": {"0": {}, "1": {}}}"#,
            )
            .unwrap();
            std::fs::write(
                output.join(format!("{REPO}.symbols.json")),
                b"{\"symbols\":1}",
            )
            .unwrap();
            for id in [0, 1] {
                std::fs::write(
                    output.join(format!("{REPO}.symbols/{id}.json")),
                    format!("{{\"district\":{id}}}"),
                )
                .unwrap();
            }
            std::fs::write(
                output.join(format!("{REPO}.names.json")),
                br#"{"0123456789ab": {"name": "core", "district": 0, "size": 3}}"#,
            )
            .unwrap();
            std::fs::write(output.join("worker-names-input.json"), b"{}").unwrap();
            std::fs::create_dir_all(dir.path().join("maps")).unwrap();
            let staging = dir.path().join("maps/.job");
            create_private_dir(&staging).unwrap();
            let owner = std::fs::metadata(dir.path()).unwrap().uid();
            Job {
                dir,
                output,
                staging,
                owner,
                files: FILES,
                districts: DISTRICTS,
            }
        }

        fn path(&self, name: &str) -> String {
            self.output.join(name).to_string_lossy().into_owned()
        }

        fn adopt_with(&self, map_path: &str) -> Result<Adopted, Refused> {
            let symbols_path = self.path(&format!("{REPO}.symbols.json"));
            let symbols_dir = self.path(&format!("{REPO}.symbols"));
            let names_cache = self.path(&format!("{REPO}.names.json"));
            adopt(
                &self.output,
                &self.staging,
                REPO,
                &Reported {
                    map_path,
                    symbols_path: &symbols_path,
                    symbols_dir: &symbols_dir,
                    names_cache: &names_cache,
                    files: self.files,
                    districts: self.districts,
                },
                Some(self.owner),
            )
        }

        fn adopt(&self) -> Result<Adopted, Refused> {
            self.adopt_with(&self.path(&format!("{REPO}.json")))
        }

        /// A file outside the job the worker should never be able to name.
        fn outside(&self) -> PathBuf {
            let path = self.dir.path().join("outside.json");
            std::fs::write(&path, b"keep me").unwrap();
            path
        }
    }

    fn refused(result: Result<Adopted, Refused>) -> String {
        match result {
            Err(Refused::Invalid(message)) => message,
            Err(Refused::Io(error)) => panic!("expected a refusal, got an I/O error: {error}"),
            Ok(_) => panic!("expected a refusal, the result was accepted"),
        }
    }

    #[test]
    fn an_early_map_is_read_from_a_regular_file_the_worker_owns() {
        let job = Job::new();
        let path = job.output.join(format!("{REPO}.json"));
        let map = read_early_map(&path, Some(job.owner)).unwrap();
        assert_eq!(map, std::fs::read(&path).unwrap());
        assert!(path.is_file(), "an early read leaves the map where it is");
    }

    #[test]
    fn an_early_map_that_is_not_a_map_or_not_the_workers_is_refused() {
        fn refused_early(result: Result<Vec<u8>, Refused>) -> String {
            match result {
                Err(refused) => refused.to_string(),
                Ok(_) => panic!("expected the early map to be refused"),
            }
        }
        let job = Job::new();
        let path = job.output.join(format!("{REPO}.json"));

        std::fs::write(&path, b"not json").unwrap();
        let message = refused_early(read_early_map(&path, Some(job.owner)));
        assert!(message.contains("not a valid map document"), "{message}");

        std::fs::write(&path, br#"{"F": [], "districts": {}}"#).unwrap();
        let message = refused_early(read_early_map(&path, Some(job.owner + 1)));
        assert!(message.contains("not owned by the worker"), "{message}");

        // A symlink at the last component is never followed.
        std::fs::remove_file(&path).unwrap();
        symlink(job.outside(), &path).unwrap();
        assert!(read_early_map(&path, Some(job.owner)).is_err());

        // A second name for a file elsewhere is not something the worker wrote.
        std::fs::remove_file(&path).unwrap();
        let other = job.dir.path().join("other.json");
        std::fs::write(&other, br#"{"F": [], "districts": {}}"#).unwrap();
        std::fs::hard_link(&other, &path).unwrap();
        let message = refused_early(read_early_map(&path, Some(job.owner)));
        assert!(message.contains("more than one link"), "{message}");

        // A FIFO neither blocks the read nor passes for a map.
        std::fs::remove_file(&path).unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let message = refused_early(read_early_map(&path, Some(job.owner)));
        assert!(message.contains("not a regular file"), "{message}");
    }

    #[test]
    fn object_ids_are_forty_or_sixty_four_lowercase_hex_digits() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"0123456789abcdef".repeat(4)));
        assert!(!is_object_id(""));
        assert!(!is_object_id(&"a".repeat(39)));
        assert!(!is_object_id(&"a".repeat(41)));
        assert!(!is_object_id(&"A".repeat(40)));
        assert!(!is_object_id(&"g".repeat(40)));
        assert!(!is_object_id(&format!("../../{}", "a".repeat(34))));
        assert!(!is_object_id(&format!("{}/x", "a".repeat(38))));
    }

    #[test]
    fn a_well_formed_result_is_moved_into_staging() {
        let job = Job::new();
        let adopted = job.adopt().unwrap();
        assert_eq!(
            std::fs::read(&adopted.map).unwrap(),
            br#"{"F": ["a.py", "b.py", "c.py"], "districts": {"0": {}, "1": {}}}"#
        );
        assert_eq!(std::fs::read(&adopted.symbols).unwrap(), b"{\"symbols\":1}");
        assert!(adopted.symbols_dir.join("0.json").is_file());
        assert!(adopted.symbols_dir.join("1.json").is_file());
        assert!(adopted.map.starts_with(&job.staging));
        assert!(adopted.symbols_dir.starts_with(&job.staging));
        assert_eq!(adopted.names["0123456789ab"].name, "core");
        assert!(!job.output.exists(), "the output directory was taken whole");
    }

    #[test]
    fn a_missing_or_unparsable_names_cache_is_an_empty_one() {
        let job = Job::new();
        std::fs::write(job.output.join(format!("{REPO}.names.json")), b"not json").unwrap();
        assert!(job.adopt().unwrap().names.is_empty());

        let job = Job::new();
        std::fs::remove_file(job.output.join(format!("{REPO}.names.json"))).unwrap();
        assert!(job.adopt().unwrap().names.is_empty());
    }

    #[test]
    fn a_reported_files_count_disagreeing_with_the_map_is_refused() {
        let mut job = Job::new();
        job.files = FILES + 1;
        let message = refused(job.adopt());
        assert!(message.contains("files"), "{message}");
    }

    #[test]
    fn a_reported_districts_count_disagreeing_with_the_map_is_refused() {
        let mut job = Job::new();
        job.districts = DISTRICTS + 1;
        let message = refused(job.adopt());
        assert!(message.contains("districts"), "{message}");
    }

    #[test]
    fn a_reported_path_outside_the_output_directory_is_refused() {
        let job = Job::new();
        let outside = job.outside();
        refused(job.adopt_with(&outside.to_string_lossy()));
        refused(job.adopt_with(&job.path(&format!("../{REPO}.json"))));
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
        assert!(job.output.join(format!("{REPO}.json")).is_file());
    }

    #[test]
    fn a_symlinked_map_file_is_refused_and_its_target_left_alone() {
        let job = Job::new();
        let outside = job.outside();
        let map = job.output.join(format!("{REPO}.json"));
        std::fs::remove_file(&map).unwrap();
        symlink(&outside, &map).unwrap();
        let message = refused(job.adopt());
        assert!(message.contains("the map"), "{message}");
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
    }

    #[test]
    fn a_symlink_inside_the_symbols_directory_is_refused() {
        let job = Job::new();
        let outside = job.outside();
        let district = job.output.join(format!("{REPO}.symbols/1.json"));
        std::fs::remove_file(&district).unwrap();
        symlink(&outside, &district).unwrap();
        refused(job.adopt());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");

        // Under a name the worker never writes, too.
        let job = Job::new();
        let outside = job.outside();
        symlink(&outside, job.output.join(format!("{REPO}.symbols/extra"))).unwrap();
        refused(job.adopt());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
    }

    #[test]
    fn a_symlinked_symbols_directory_is_refused() {
        let job = Job::new();
        let elsewhere = job.dir.path().join("elsewhere");
        std::fs::rename(job.output.join(format!("{REPO}.symbols")), &elsewhere).unwrap();
        symlink(&elsewhere, job.output.join(format!("{REPO}.symbols"))).unwrap();
        refused(job.adopt());
        assert!(elsewhere.join("0.json").is_file());
        assert!(elsewhere.join("1.json").is_file());
    }

    #[test]
    fn a_symlinked_output_directory_is_refused_and_its_target_left_alone() {
        let job = Job::new();
        let elsewhere = job.dir.path().join("elsewhere");
        std::fs::rename(&job.output, &elsewhere).unwrap();
        symlink(&elsewhere, &job.output).unwrap();
        refused(job.adopt());
        assert!(elsewhere.join(format!("{REPO}.json")).is_file());
        assert!(elsewhere.join(format!("{REPO}.symbols/0.json")).is_file());
    }

    #[test]
    fn a_hard_linked_map_file_is_refused() {
        let job = Job::new();
        std::fs::hard_link(
            job.output.join(format!("{REPO}.json")),
            job.dir.path().join("second-name.json"),
        )
        .unwrap();
        let message = refused(job.adopt());
        assert!(message.contains("more than one link"), "{message}");
    }

    #[test]
    fn a_file_the_worker_does_not_own_is_refused() {
        let job = Job::new();
        let symbols_path = job.path(&format!("{REPO}.symbols.json"));
        let symbols_dir = job.path(&format!("{REPO}.symbols"));
        let names_cache = job.path(&format!("{REPO}.names.json"));
        let map_path = job.path(&format!("{REPO}.json"));
        let result = adopt(
            &job.output,
            &job.staging,
            REPO,
            &Reported {
                map_path: &map_path,
                symbols_path: &symbols_path,
                symbols_dir: &symbols_dir,
                names_cache: &names_cache,
                files: job.files,
                districts: job.districts,
            },
            Some(job.owner.wrapping_add(1)),
        );
        let message = refused(result);
        assert!(message.contains("not owned by the worker"), "{message}");
    }

    #[test]
    fn a_store_file_is_copied_for_the_worker_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("stored.json");
        std::fs::write(&source, b"{\"map\":1}").unwrap();
        let dest = dir.path().join("copy.json");
        assert_eq!(copy_for_worker(&source, &dest).unwrap(), 9);
        assert_eq!(std::fs::read(&dest).unwrap(), b"{\"map\":1}");
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_copy_for_the_worker_follows_no_symlink_and_overwrites_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.json");
        std::fs::write(&outside, b"keep me").unwrap();

        // A symlinked source is not read through.
        let linked_source = dir.path().join("linked.json");
        symlink(&outside, &linked_source).unwrap();
        assert!(copy_for_worker(&linked_source, &dir.path().join("a.json")).is_err());
        assert!(!dir.path().join("a.json").exists());

        // A directory is not a map.
        assert!(copy_for_worker(dir.path(), &dir.path().join("b.json")).is_err());

        // A symlink already at the destination is not written through.
        let source = dir.path().join("stored.json");
        std::fs::write(&source, b"{\"map\":1}").unwrap();
        let linked_dest = dir.path().join("dest.json");
        symlink(&outside, &linked_dest).unwrap();
        assert!(copy_for_worker(&source, &linked_dest).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");

        // Nor is an existing file replaced.
        assert!(copy_for_worker(&source, &outside).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
    }

    #[test]
    fn a_repository_name_that_is_not_one_file_name_is_refused() {
        assert!(OutputNames::for_repo("a/b").is_err());
        assert!(OutputNames::for_repo("demo").is_ok());
    }
}
