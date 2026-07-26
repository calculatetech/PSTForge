use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn configure_libpff_runtime_path() -> Result<(), String> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../build-support/libpff_rpath.rs");
    println!("cargo:rerun-if-changed=../../third_party/libpff");
    println!("cargo:rerun-if-env-changed=PSTFORGE_LIBPFF_PREFIX");
    println!("cargo:rerun-if-env-changed=PSTFORGE_LIBPFF_RPATH");

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
        let head_output = Command::new("git")
            .args([
                OsStr::new("-C"),
                source.as_os_str(),
                OsStr::new("rev-parse"),
                OsStr::new("--git-path"),
                OsStr::new("HEAD"),
            ])
            .output()
            .map_err(|error| format!("cannot locate the libpff Git HEAD: {error}"))?;
        if !head_output.status.success() {
            return Err(format!(
                "git rev-parse --git-path HEAD failed for {} with {}",
                source.display(),
                head_output.status
            ));
        }
        let head_path = String::from_utf8(head_output.stdout)
            .map_err(|_| "libpff Git HEAD path is not UTF-8".to_owned())?;
        let head_path = PathBuf::from(head_path.trim());
        let head_path = if head_path.is_absolute() {
            head_path
        } else {
            source.join(head_path)
        };
        if !head_path.is_file() {
            return Err(format!(
                "cannot resolve the libpff Git HEAD at {}",
                head_path.display()
            ));
        }
        println!("cargo:rerun-if-changed={}", head_path.display());
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
