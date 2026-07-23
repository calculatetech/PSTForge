use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/bindings.rs");
    println!("cargo:rerun-if-changed=../../.gitmodules");
    println!("cargo:rerun-if-changed=../../third_party/libpff");
    println!("cargo:rerun-if-env-changed=PSTFORGE_LIBPFF_PREFIX");

    if let Err(error) = configure() {
        eprintln!("cannot build the pinned PSTForge libpff runtime: {error}");
        std::process::exit(1);
    }
}

fn configure() -> Result<(), String> {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| "CARGO_MANIFEST_DIR is not set".to_owned())?,
    );
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot resolve the workspace root".to_owned())?;
    let source = root.join("third_party/libpff");
    if !source.join("configure.ac").is_file() {
        return Err(format!(
            "{} is unavailable; initialize submodules with `git submodule update --init --recursive`",
            source.display()
        ));
    }
    let revision = command_output(
        root,
        "git",
        [
            OsStr::new("-C"),
            source.as_os_str(),
            OsStr::new("rev-parse"),
            OsStr::new("HEAD"),
        ],
    )?;
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("unexpected libpff revision: {revision:?}"));
    }

    let prefix = if let Some(prefix) = env::var_os("PSTFORGE_LIBPFF_PREFIX") {
        PathBuf::from(prefix)
    } else {
        let native_root = root.join("target/native/libpff");
        let build = native_root.join(revision);
        let prefix = build.join("install");
        if !prefix.join("lib/libpff.so.1").is_file() {
            build_runtime(&source, &native_root, &build, revision)?;
        }
        prefix
    };
    let library_directory = prefix.join("lib");
    if !library_directory.join("libpff.so.1").is_file() {
        return Err(format!(
            "{} does not contain lib/libpff.so.1",
            prefix.display()
        ));
    }
    println!(
        "cargo:rustc-link-search=native={}",
        library_directory.display()
    );
    println!("cargo:rustc-link-lib=dylib=pff");
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        library_directory.display()
    );
    println!("cargo:rustc-env=PSTFORGE_LIBPFF_REVISION={revision}");
    Ok(())
}

fn build_runtime(
    source: &Path,
    native_root: &Path,
    build: &Path,
    revision: &str,
) -> Result<(), String> {
    fs::create_dir_all(native_root)
        .map_err(|error| format!("cannot create {}: {error}", native_root.display()))?;
    if build.exists() {
        return Err(format!(
            "incomplete native build at {}; remove that exact directory and retry",
            build.display()
        ));
    }
    let temporary = native_root.join(format!(".{revision}.{}.tmp", std::process::id()));
    if temporary.exists() {
        return Err(format!(
            "temporary native build already exists at {}",
            temporary.display()
        ));
    }
    fs::create_dir(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let archive = temporary.join("source.tar");
    run(
        source,
        "git",
        [
            OsStr::new("archive"),
            OsStr::new("--format=tar"),
            OsStr::new("--output"),
            archive.as_os_str(),
            OsStr::new("HEAD"),
        ],
    )?;
    let extracted = temporary.join("source");
    let temporary_prefix = temporary.join("install");
    fs::create_dir(&extracted)
        .map_err(|error| format!("cannot create {}: {error}", extracted.display()))?;
    run(&extracted, "tar", [OsStr::new("-xf"), archive.as_os_str()])?;
    run(&extracted, "./autogen.sh", std::iter::empty::<&OsStr>())?;
    let prefix_argument = format!("--prefix={}", temporary_prefix.display());
    run(
        &extracted,
        "./configure",
        [
            OsStr::new("--disable-static"),
            OsStr::new("--disable-nls"),
            OsStr::new(&prefix_argument),
        ],
    )?;
    let jobs = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .to_string();
    run(&extracted, "make", [OsStr::new("-j"), OsStr::new(&jobs)])?;
    run(&extracted, "make", [OsStr::new("install")])?;
    if !temporary_prefix.join("lib/libpff.so.1").is_file() {
        return Err("native build completed without lib/libpff.so.1".to_owned());
    }
    fs::write(temporary.join("REVISION"), format!("{revision}\n"))
        .map_err(|error| format!("cannot record libpff revision: {error}"))?;
    fs::remove_file(&archive)
        .map_err(|error| format!("cannot remove {}: {error}", archive.display()))?;
    fs::rename(&temporary, build).map_err(|error| {
        format!(
            "cannot publish native build {} as {}: {error}",
            temporary.display(),
            build.display()
        )
    })?;
    Ok(())
}

fn run<I, S>(directory: &Path, program: &str, arguments: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(program)
        .args(arguments)
        .current_dir(directory)
        .status()
        .map_err(|error| format!("cannot run {program} in {}: {error}", directory.display()))?;
    check_status(program, directory, status)
}

fn command_output<I, S>(directory: &Path, program: &str, arguments: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program)
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("cannot run {program} in {}: {error}", directory.display()))?;
    check_status(program, directory, output.status)?;
    String::from_utf8(output.stdout).map_err(|_| format!("{program} output is not UTF-8"))
}

fn check_status(program: &str, directory: &Path, status: ExitStatus) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed with {status} in {}",
            directory.display()
        ))
    }
}
