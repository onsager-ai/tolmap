use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=native/leiden_bridge.cpp");
    println!("cargo:rerun-if-env-changed=LEIDEN_PREFIX");

    let prefix = env::var_os("LEIDEN_PREFIX")
        .map(PathBuf::from)
        .or_else(discover_system_prefix)
        .unwrap_or_else(|| {
            panic!(
                "libleidenalg/igraph not found; run scripts/install-leiden.sh and set LEIDEN_PREFIX"
            )
        });
    let include = prefix.join("include");
    let lib = prefix.join("lib");
    if !include.join("libleidenalg/Optimiser.h").is_file()
        || !include.join("igraph/igraph.h").is_file()
    {
        panic!("LEIDEN_PREFIX={} has no compatible headers", prefix.display());
    }

    cc::Build::new()
        .cpp(true)
        .std("c++14")
        .include(&include)
        .file("native/leiden_bridge.cpp")
        .warnings(false)
        .compile("tolmap_leiden_bridge");

    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=dylib=libleidenalg");
    println!("cargo:rustc-link-lib=dylib=igraph");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
}

fn discover_system_prefix() -> Option<PathBuf> {
    ["/usr/local", "/usr"]
        .into_iter()
        .map(Path::new)
        .find(|p| p.join("include/libleidenalg/Optimiser.h").is_file())
        .map(Path::to_path_buf)
}
