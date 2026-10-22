use std::{fs, path::Path, process::Command};
use tolmap::source;
fn git(repo: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}
#[test]
fn objects_preserve_a_after_b_and_refuse_boundaries() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    git(p, &["init"]);
    git(p, &["config", "user.email", "fixture@example.invalid"]);
    git(p, &["config", "user.name", "fixture"]);
    fs::write(
        p.join("code.py"),
        "# <script>alert('inert')</script>\ndef alpha():\n    return 'A'\n",
    )
    .unwrap();
    fs::write(p.join("binary"), [0, 1, 2]).unwrap();
    for name in ["quote\".txt", "tab\tname.txt", "源码.txt"] {
        fs::write(p.join(name), "inert text").unwrap();
    }
    fs::write(p.join("large"), vec![b'a'; source::MAX_BYTES as usize + 1]).unwrap();
    fs::write(p.join("long"), "a".repeat(source::LONG_LINE + 1)).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", p.join("escape")).unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "A"]);
    let a = git(p, &["rev-parse", "HEAD"]);
    fs::write(p.join("code.py"), "B\n").unwrap();
    git(p, &["commit", "-am", "B"]);
    let b = git(p, &["rev-parse", "HEAD"]);
    assert!(source::read(p, "repo", &a, "code.py")
        .unwrap()
        .text
        .unwrap()
        .contains("return 'A'"));
    assert_eq!(
        source::read(p, "repo", &b, "code.py")
            .unwrap()
            .text
            .unwrap(),
        "B\n"
    );
    for name in ["quote\".txt", "tab\tname.txt", "源码.txt"] {
        assert_eq!(
            source::read(p, "repo", &a, name).unwrap().text.unwrap(),
            "inert text"
        );
    }
    assert_eq!(
        source::read(p, "repo", &a, "binary").unwrap().status,
        "binary"
    );
    assert_eq!(
        source::read(p, "repo", &a, "large").unwrap().status,
        "large"
    );
    assert!(source::read(p, "repo", &a, "long").unwrap().long_line);
    assert_eq!(
        source::read(p, "repo", &a, "missing").unwrap().status,
        "missing"
    );
    #[cfg(unix)]
    assert_eq!(
        source::read(p, "repo", &a, "escape").unwrap().status,
        "unsupported"
    );
    assert_eq!(
        source::read(p, "repo", &"0".repeat(40), "code.py")
            .unwrap()
            .status,
        "expired"
    );
    for path in ["../secret", "/etc/passwd", "escape/other", ".git/config"] {
        let result = source::read(p, "repo", &a, path);
        assert!(result.is_err() || result.unwrap().text.is_none());
    }
    assert!(source::read(p, "repo", "HEAD", "code.py").is_err());
}
