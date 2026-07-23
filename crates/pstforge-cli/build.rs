use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../third_party/libpff");
    println!("cargo:rerun-if-env-changed=PSTFORGE_LIBPFF_PREFIX");
    println!("cargo:rerun-if-env-changed=PSTFORGE_LIBPFF_RPATH");

    if let Err(error) = configure_runtime_path() {
        eprintln!("cannot configure the pinned libpff runtime path: {error}");
        std::process::exit(1);
    }
}

fn configure_runtime_path() -> Result<(), String> {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| "CARGO_MANIFEST_DIR is not set".to_owned())?,
    );
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot resolve the workspace root".to_owned())?;
    let source = root.join("third_party/libpff");
    let runtime_path = if let Some(path) = env::var_os("PSTFORGE_LIBPFF_RPATH") {
        path.into_string()
            .map_err(|_| "PSTFORGE_LIBPFF_RPATH is not valid UTF-8".to_owned())?
    } else if let Some(prefix) = env::var_os("PSTFORGE_LIBPFF_PREFIX") {
        PathBuf::from(prefix)
            .join("lib")
            .to_string_lossy()
            .into_owned()
    } else {
        let output = Command::new("git")
            .args([
                OsStr::new("-C"),
                source.as_os_str(),
                OsStr::new("rev-parse"),
                OsStr::new("HEAD"),
            ])
            .output()
            .map_err(|error| format!("cannot inspect the libpff submodule: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git rev-parse failed for {} with {}",
                source.display(),
                output.status
            ));
        }
        let revision = String::from_utf8(output.stdout)
            .map_err(|_| "libpff revision is not UTF-8".to_owned())?;
        let revision = revision.trim();
        if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("unexpected libpff revision: {revision:?}"));
        }
        root.join("target/native/libpff")
            .join(revision)
            .join("install/lib")
            .to_string_lossy()
            .into_owned()
    };
    if runtime_path.contains(['\n', '\r']) {
        return Err("libpff runtime path contains a line break".to_owned());
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,{runtime_path}");
    Ok(())
}
