#![deny(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read as _;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CorpusManifest {
    schema_version: u32,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CorpusCase {
    name: String,
    path: PathBuf,
    sha256: String,
    classification: String,
    milestone_0_1: bool,
    #[serde(default)]
    milestone_0_1_1: bool,
    minimum_folders: u64,
    minimum_messages: u64,
    #[serde(default)]
    #[serde(rename = "minimum_recipients")]
    _minimum_recipients: u64,
    #[serde(default)]
    #[serde(rename = "minimum_attachments")]
    _minimum_attachments: u64,
    #[serde(default)]
    #[serde(rename = "minimum_raw_properties")]
    _minimum_raw_properties: u64,
    #[serde(default = "default_peak_chunk_limit")]
    maximum_peak_stream_chunk_bytes: u64,
    #[serde(default)]
    #[serde(rename = "milestone_0_3")]
    _milestone_0_3: bool,
    #[serde(default)]
    #[serde(rename = "minimum_recovered_items")]
    _minimum_recovered_items: u64,
    #[serde(default)]
    #[serde(rename = "minimum_orphan_items")]
    _minimum_orphan_items: u64,
    #[serde(default)]
    milestone_0_4: bool,
    #[serde(default = "default_split_limit")]
    milestone_0_4_max_pst_bytes: u64,
    #[serde(default)]
    milestone_0_4_allow_oversize: bool,
}

fn default_split_limit() -> u64 {
    2 * 1024 * 1024
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    resolve: Option<CargoResolve>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
    version: String,
    license: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    manifest_path: PathBuf,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CargoResolve {
    nodes: Vec<CargoNode>,
}

#[derive(Debug, Deserialize)]
struct CargoNode {
    id: String,
    deps: Vec<CargoNodeDependency>,
}

#[derive(Debug, Deserialize)]
struct CargoNodeDependency {
    pkg: String,
    dep_kinds: Vec<CargoDependencyKind>,
}

#[derive(Debug, Deserialize)]
struct CargoDependencyKind {
    kind: Option<String>,
}

struct Gate {
    root: PathBuf,
    evidence: PathBuf,
}

struct PreparedCorpus {
    manifest: CorpusManifest,
    manifest_path: PathBuf,
    _scratch: tempfile::TempDir,
}

#[derive(Default)]
struct IndependentFidelitySink {
    top_level: Option<u64>,
    embedded: Option<u64>,
    top_level_id: Option<u32>,
    embedded_id: Option<u32>,
    recipients: Vec<(u32, Option<u32>, Option<String>)>,
    properties: Vec<(libpff_sys::PropertyDescriptor, Vec<u8>)>,
}

impl IndependentFidelitySink {
    fn property(&self, message_id: u32, property_id: u32) -> Result<&[u8], String> {
        let mut matches = self.properties.iter().filter(|(descriptor, _)| {
            descriptor.owner == libpff_sys::PropertyOwner::Message(message_id)
                && descriptor.entry_type == Some(property_id)
        });
        let (_, bytes) = matches
            .next()
            .ok_or_else(|| format!("libpff did not expose property 0x{property_id:04X}"))?;
        if matches.next().is_some() {
            return Err(format!(
                "libpff exposed duplicate property 0x{property_id:04X}"
            ));
        }
        Ok(bytes)
    }

    fn property_type(&self, message_id: u32, property_id: u32) -> Option<u32> {
        self.properties
            .iter()
            .find(|(descriptor, _)| {
                descriptor.owner == libpff_sys::PropertyOwner::Message(message_id)
                    && descriptor.entry_type == Some(property_id)
            })
            .and_then(|(descriptor, _)| descriptor.value_type)
    }
}

impl libpff_sys::CatalogSink for IndependentFidelitySink {
    fn event(&mut self, event: libpff_sys::CatalogEvent<'_>) -> Result<(), String> {
        match event {
            libpff_sys::CatalogEvent::MessageStart {
                id,
                parent_message_id,
                delivery_filetime,
                ..
            } => {
                let (time_slot, id_slot) = if parent_message_id.is_some() {
                    (&mut self.embedded, &mut self.embedded_id)
                } else {
                    (&mut self.top_level, &mut self.top_level_id)
                };
                if time_slot
                    .replace(delivery_filetime.ok_or_else(|| {
                        "libpff did not expose the message delivery time".to_owned()
                    })?)
                    .is_some()
                    || id_slot.replace(id).is_some()
                {
                    return Err("libpff exposed an unexpected additional message".to_owned());
                }
            }
            libpff_sys::CatalogEvent::Recipient {
                message_id,
                recipient_type,
                email_address,
                ..
            } => self
                .recipients
                .push((message_id, recipient_type, email_address)),
            libpff_sys::CatalogEvent::PropertyStart(descriptor)
                if matches!(descriptor.owner, libpff_sys::PropertyOwner::Message(_)) =>
            {
                self.properties.push((descriptor, Vec::new()));
            }
            libpff_sys::CatalogEvent::PropertyData { descriptor, bytes }
                if matches!(descriptor.owner, libpff_sys::PropertyOwner::Message(_)) =>
            {
                let (_, output) = self
                    .properties
                    .iter_mut()
                    .rev()
                    .find(|(candidate, _)| *candidate == descriptor)
                    .ok_or_else(|| {
                        "libpff emitted property data before its descriptor".to_owned()
                    })?;
                output.extend_from_slice(bytes);
            }
            _ => {}
        }
        Ok(())
    }
}

fn utf16le(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn validate_independent_properties(sink: &IndependentFidelitySink) -> Result<(), String> {
    const PT_LONG: u32 = 0x0003;
    const PT_BOOLEAN: u32 = 0x000b;
    const PT_UNICODE: u32 = 0x001f;
    const PT_CLSID: u32 = 0x0048;

    let top = sink
        .top_level_id
        .ok_or_else(|| "libpff did not expose the top-level message ID".to_owned())?;
    let embedded = sink
        .embedded_id
        .ok_or_else(|| "libpff did not expose the embedded message ID".to_owned())?;
    let checks = [
        (
            top,
            0x8000,
            PT_UNICODE,
            utf16le("named property checkpoint"),
        ),
        (top, 0x8002, PT_LONG, 21_i32.to_le_bytes().to_vec()),
        (embedded, 0x8001, PT_BOOLEAN, vec![1]),
        (top, 0x10f4, PT_UNICODE, utf16le("raw property checkpoint")),
        (top, 0x10f5, PT_CLSID, b"PSTForgeRawGuid!".to_vec()),
    ];
    for (message, id, expected_type, expected) in checks {
        if sink.property_type(message, id) != Some(expected_type) {
            return Err(format!("libpff property 0x{id:04X} type mismatch"));
        }
        let actual = sink.property(message, id)?;
        if actual != expected {
            return Err(format!(
                "libpff property 0x{id:04X} bytes mismatch: expected {expected:02x?}, got {actual:02x?}"
            ));
        }
    }
    if !sink.recipients.iter().any(|(message, kind, address)| {
        *message == top && *kind == Some(3) && address.as_deref() == Some("bcc@example.com")
    }) {
        return Err("libpff did not expose the top-level Bcc recipient role/address".to_owned());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot locate workspace root".to_owned())?
        .to_path_buf();
    let command = arguments.next().ok_or_else(usage)?;
    if command == OsStr::new("package") {
        let format = arguments
            .next()
            .ok_or_else(|| "missing package format: deb".to_owned())?;
        if format != OsStr::new("deb") || arguments.next().is_some() {
            return Err("usage: cargo xtask package deb".to_owned());
        }
        return package_deb(&root);
    }
    if command == std::ffi::OsStr::new("qualify") {
        let checkpoint = arguments
            .next()
            .ok_or_else(|| "missing qualification checkpoint".to_owned())?;
        let output = arguments
            .next()
            .ok_or_else(|| "missing qualification output directory".to_owned())?;
        if arguments.next().is_some() {
            return Err("unexpected arguments after qualification output directory".to_owned());
        }
        return match checkpoint.to_str() {
            Some("embedded-attachments") => qualify_embedded_attachments(&root, Path::new(&output)),
            Some("named-properties") => qualify_named_properties(&root, Path::new(&output)),
            Some("empty-folders") => qualify_empty_folders(&root, Path::new(&output)),
            Some("contacts") => qualify_contacts(&root, Path::new(&output)),
            Some("appointments") => qualify_appointments(&root, Path::new(&output)),
            Some("meetings") => qualify_meetings(&root, Path::new(&output)),
            Some("pim-items") => qualify_pim_items(&root, Path::new(&output)),
            Some("distribution-list") => qualify_distribution_list(&root, Path::new(&output)),
            Some("document-object") => qualify_document_object(&root, Path::new(&output)),
            Some("ole-attachments") => qualify_ole_attachments(&root, Path::new(&output)),
            Some("reference-attachments") => {
                qualify_reference_attachments(&root, Path::new(&output))
            }
            Some("calendar-exceptions") => {
                qualify_calendar_exceptions(&root, Path::new(&output))
            }
            Some("associated-data") => qualify_associated_data(&root, Path::new(&output)),
            _ => Err("unknown qualification checkpoint; expected embedded-attachments, named-properties, empty-folders, contacts, appointments, meetings, pim-items, distribution-list, document-object, ole-attachments, reference-attachments, calendar-exceptions, or associated-data".to_owned()),
        };
    }
    if command != std::ffi::OsStr::new("gate") {
        return Err(usage());
    }
    let tier = arguments
        .next()
        .ok_or_else(|| "missing gate tier: fast, full, or release".to_owned())?;
    if arguments.next().is_some() {
        return Err("unexpected arguments after gate tier".to_owned());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_secs();
    let evidence = root
        .join(".agent/test-results")
        .join(format!("{stamp}-{}", tier.to_string_lossy()));
    fs::create_dir_all(&evidence)
        .map_err(|error| format!("cannot create {}: {error}", evidence.display()))?;
    let gate = Gate { root, evidence };

    match tier.to_str() {
        Some("fast") => gate.fast(),
        Some("full") => gate.full(),
        Some("release") => gate.release(),
        _ => Err("unknown gate tier; expected fast, full, or release".to_owned()),
    }
}

fn usage() -> String {
    "usage: cargo xtask gate <fast|full|release> | package deb | qualify <embedded-attachments|named-properties|empty-folders|contacts|appointments|meetings|pim-items|distribution-list|document-object|ole-attachments|reference-attachments|calendar-exceptions|associated-data> <output>"
        .to_owned()
}

fn package_deb(root: &Path) -> Result<(), String> {
    for program in [
        "dpkg",
        "dpkg-deb",
        "dpkg-shlibdeps",
        "find",
        "gzip",
        "lintian",
        "readelf",
        "strip",
        "touch",
    ] {
        require_program(program)?;
    }
    let architecture = command_stdout(root, "dpkg", &["--print-architecture"])?;
    if architecture.trim() != "amd64" {
        return Err(format!(
            "Debian packaging currently supports amd64, not {}",
            architecture.trim()
        ));
    }
    let version = env!("CARGO_PKG_VERSION");
    let epoch = source_date_epoch(root)?;
    let package_root = root.join("target/debian");
    prepare_generated_directory(root, &package_root)?;
    fs::create_dir_all(&package_root)
        .map_err(|error| format!("cannot create {}: {error}", package_root.display()))?;
    let first_target = package_root.join("build-one");
    let second_target = package_root.join("build-two");
    build_release_binary(root, &first_target, &epoch)?;
    build_release_binary(root, &second_target, &epoch)?;
    let first_binary = first_target.join("release/pstforge");
    let second_binary = second_target.join("release/pstforge");
    let dependencies = debian_runtime_dependencies(&package_root, &first_binary)?;
    let rust_licenses = rust_dependency_licenses(root)?;
    let first = package_root.join(format!("pstforge_{version}_amd64.first.deb"));
    let second = package_root.join(format!("pstforge_{version}_amd64.second.deb"));
    build_deb_archive(
        root,
        &first_binary,
        &dependencies,
        &rust_licenses,
        &epoch,
        "one",
        &first,
    )?;
    build_deb_archive(
        root,
        &second_binary,
        &dependencies,
        &rust_licenses,
        &epoch,
        "two",
        &second,
    )?;
    let first_bytes =
        fs::read(&first).map_err(|error| format!("cannot read {}: {error}", first.display()))?;
    if first_bytes
        != fs::read(&second)
            .map_err(|error| format!("cannot read {}: {error}", second.display()))?
    {
        return Err("two Debian package builds were not byte-for-byte reproducible".to_owned());
    }
    validate_deb_package(root, &package_root, &first, version, &dependencies)?;
    let package = package_root.join(format!("pstforge_{version}_amd64.deb"));
    fs::rename(&first, &package)
        .map_err(|error| format!("cannot publish {}: {error}", package.display()))?;
    fs::remove_file(&second)
        .map_err(|error| format!("cannot remove {}: {error}", second.display()))?;
    println!(
        "Debian package: {}\nsha256: {:x}\ndepends: {}",
        package.display(),
        Sha256::digest(&first_bytes),
        dependencies
    );
    Ok(())
}

fn build_release_binary(root: &Path, target: &Path, epoch: &str) -> Result<(), String> {
    let remap = format!("--remap-path-prefix={}=/usr/src/pstforge", root.display());
    let status = Command::new("cargo")
        .args(["build", "--locked", "--release", "-p", "pstforge-cli"])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTFLAGS", remap)
        .env("SOURCE_DATE_EPOCH", epoch)
        .current_dir(root)
        .status()
        .map_err(|error| format!("cannot run reproducible Cargo build: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reproducible Cargo build failed with {status}"))
    }
}

fn require_program(program: &str) -> Result<(), String> {
    let status = Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("required packaging tool {program} is unavailable: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("required packaging tool {program} is not usable"))
    }
}

fn source_date_epoch(root: &Path) -> Result<String, String> {
    if let Some(value) = std::env::var_os("SOURCE_DATE_EPOCH") {
        let value = value
            .into_string()
            .map_err(|_| "SOURCE_DATE_EPOCH is not valid UTF-8".to_owned())?;
        value
            .parse::<u64>()
            .map_err(|_| "SOURCE_DATE_EPOCH must be an unsigned Unix timestamp".to_owned())?;
        return Ok(value);
    }
    Ok(command_stdout(root, "git", &["log", "-1", "--format=%ct"])?
        .trim()
        .to_owned())
}

fn debian_runtime_dependencies(package_root: &Path, binary: &Path) -> Result<String, String> {
    let work = package_root.join("shlibdeps");
    fs::create_dir_all(work.join("debian"))
        .map_err(|error| format!("cannot create dependency workspace: {error}"))?;
    fs::write(
        work.join("debian/control"),
        "Source: pstforge\nSection: utils\nPriority: optional\nMaintainer: PSTForge Maintainers <noreply@pstforge.invalid>\nStandards-Version: 4.7.0\n\nPackage: pstforge\nArchitecture: any\nDepends: ${shlibs:Depends}\nDescription: PST recovery utility\n",
    )
    .map_err(|error| format!("cannot create dependency control file: {error}"))?;
    let output = command_stdout_os(
        &work,
        OsStr::new("dpkg-shlibdeps"),
        &[
            OsString::from("-O"),
            OsString::from(format!("-e{}", binary.display())),
        ],
    )?;
    let generated = output
        .trim()
        .strip_prefix("shlibs:Depends=")
        .ok_or_else(|| "dpkg-shlibdeps returned an unexpected result".to_owned())?;
    let mut dependencies = generated
        .split(',')
        .map(str::trim)
        .filter(|dependency| !dependency.starts_with("libpff1t64 "))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    dependencies.push("libpff1t64 (>= 20180714)".to_owned());
    dependencies.sort();
    Ok(dependencies.join(", "))
}

fn rust_dependency_licenses(root: &Path) -> Result<String, String> {
    let metadata: CargoMetadata = serde_json::from_str(&command_stdout(
        root,
        "cargo",
        &[
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            "x86_64-unknown-linux-gnu",
        ],
    )?)
    .map_err(|error| format!("cannot parse Cargo metadata for licenses: {error}"))?;
    let resolve = metadata
        .resolve
        .as_ref()
        .ok_or_else(|| "Cargo metadata omitted the dependency graph".to_owned())?;
    let root_id = metadata
        .packages
        .iter()
        .find(|package| package.name == "pstforge-cli" && package.source.is_none())
        .map(|package| package.id.clone())
        .ok_or_else(|| "cannot locate pstforge-cli in Cargo metadata".to_owned())?;
    let nodes = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut pending = vec![root_id];
    let mut closure = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !closure.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .get(id.as_str())
            .ok_or_else(|| format!("Cargo dependency graph has no node for {id}"))?;
        pending.extend(
            node.deps
                .iter()
                .filter(|dependency| dependency.dep_kinds.iter().any(|kind| kind.kind.is_none()))
                .map(|dependency| dependency.pkg.clone()),
        );
    }
    let mut packages = metadata
        .packages
        .iter()
        .filter(|package| closure.contains(&package.id) && package.source.is_some())
        .collect::<Vec<_>>();
    packages.sort_by(|left, right| {
        (&left.name, &left.version, &left.id).cmp(&(&right.name, &right.version, &right.id))
    });

    let mut output = String::from(
        "Rust dependency licenses bundled with PSTForge\n================================================\n\nThis file covers third-party Rust crates linked into the pstforge executable.\n\n",
    );
    for package in packages {
        let directory = package
            .manifest_path
            .parent()
            .ok_or_else(|| format!("{} has no package directory", package.name))?;
        let mut license_files = fs::read_dir(directory)
            .map_err(|error| format!("cannot read {} license directory: {error}", package.name))?
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                entry.path().is_file()
                    && (name.starts_with("license")
                        || name.starts_with("copying")
                        || name.starts_with("notice")
                        || name.starts_with("copyright"))
            })
            .collect::<Vec<_>>();
        license_files.sort_by_key(|entry| entry.file_name());
        output.push_str(&format!(
            "{} {}\nLicense expression: {}\nAuthors: {}\nSource: {}\n",
            package.name,
            package.version,
            package.license.as_deref().unwrap_or("UNSPECIFIED"),
            if package.authors.is_empty() {
                "not stated".to_owned()
            } else {
                package.authors.join(", ")
            },
            package.source.as_deref().unwrap_or("workspace")
        ));
        if license_files.is_empty() {
            let expression = package.license.as_deref().unwrap_or("UNSPECIFIED");
            if !expression
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
                .any(|token| token == "MIT")
            {
                return Err(format!(
                    "Rust dependency {} {} ships no license file and has no selectable MIT grant",
                    package.name, package.version
                ));
            }
            let authors = if package.authors.is_empty() {
                "the crate authors".to_owned()
            } else {
                package.authors.join(", ")
            };
            let mit = fs::read_to_string(root.join("LICENSE-MIT"))
                .map_err(|error| format!("cannot read MIT license template: {error}"))?
                .replace(
                    "Copyright (c) 2026 PSTForge contributors",
                    &format!("Copyright (c) {authors}"),
                );
            output.push_str(&format!(
                "\n--- GENERATED-MIT-LICENSE ---\n{}\n",
                mit.trim_end()
            ));
        }
        for entry in license_files {
            let bytes = fs::read(entry.path()).map_err(|error| {
                format!(
                    "cannot read {} for {}: {error}",
                    entry.path().display(),
                    package.name
                )
            })?;
            let text = String::from_utf8(bytes).map_err(|_| {
                format!("{} contains non-UTF-8 license text", entry.path().display())
            })?;
            output.push_str(&format!(
                "\n--- {} ---\n{}\n",
                entry.file_name().to_string_lossy(),
                text.replace("\r\n", "\n").trim_end()
            ));
        }
        output.push_str("\n================================================\n\n");
    }
    Ok(output)
}

fn build_deb_archive(
    root: &Path,
    binary: &Path,
    dependencies: &str,
    rust_licenses: &str,
    epoch: &str,
    suffix: &str,
    output: &Path,
) -> Result<(), String> {
    let package_root = output
        .parent()
        .ok_or_else(|| "Debian output path has no parent".to_owned())?;
    let stage = package_root.join(format!("stage-{suffix}"));
    stage_deb_tree(root, &stage, binary, dependencies, rust_licenses)?;
    normalize_tree_timestamps(&stage, epoch)?;
    let status = Command::new("dpkg-deb")
        .args(["--root-owner-group", "-Zxz", "-z9", "--build"])
        .arg(&stage)
        .arg(output)
        .env("SOURCE_DATE_EPOCH", epoch)
        .current_dir(root)
        .status()
        .map_err(|error| format!("cannot run dpkg-deb: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("dpkg-deb failed with {status}"))
    }
}

fn stage_deb_tree(
    root: &Path,
    stage: &Path,
    binary: &Path,
    dependencies: &str,
    rust_licenses: &str,
) -> Result<(), String> {
    for relative in [
        "DEBIAN",
        "usr",
        "usr/bin",
        "usr/share",
        "usr/share/doc",
        "usr/share/doc/pstforge",
        "usr/share/man",
        "usr/share/man/man1",
        "usr/share/pstforge",
        "usr/share/pstforge/schemas",
    ] {
        let directory = stage.join(relative);
        fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create package staging directory: {error}"))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot set package directory permissions: {error}"))?;
    }
    let packaged_binary = stage.join("usr/bin/pstforge");
    copy_with_mode(binary, &packaged_binary, 0o755)?;
    run_checked_os(
        root,
        OsStr::new("strip"),
        &[
            OsString::from("--strip-unneeded"),
            packaged_binary.as_os_str().to_owned(),
        ],
    )?;
    for (source, destination) in [
        ("README.md", "usr/share/doc/pstforge/README.md"),
        (
            "docs/PRODUCT_SPEC.md",
            "usr/share/doc/pstforge/PRODUCT_SPEC.md",
        ),
        ("LICENSE-APACHE", "usr/share/doc/pstforge/LICENSE-APACHE"),
        ("LICENSE-MIT", "usr/share/doc/pstforge/LICENSE-MIT"),
        (
            "THIRD_PARTY_LICENSES.md",
            "usr/share/doc/pstforge/THIRD_PARTY_LICENSES.md",
        ),
        (
            "crates/pstforge-pst/LICENSE",
            "usr/share/doc/pstforge/WRITER-LICENSE-MIT",
        ),
        ("docs/debian-copyright", "usr/share/doc/pstforge/copyright"),
    ] {
        copy_with_mode(&root.join(source), &stage.join(destination), 0o644)?;
    }
    let rust_license_path = stage.join("usr/share/doc/pstforge/RUST-DEPENDENCY-LICENSES.txt");
    fs::write(&rust_license_path, rust_licenses)
        .map_err(|error| format!("cannot write Rust dependency licenses: {error}"))?;
    fs::set_permissions(&rust_license_path, fs::Permissions::from_mode(0o644))
        .map_err(|error| format!("cannot set Rust dependency license permissions: {error}"))?;
    for entry in fs::read_dir(root.join("docs/schemas"))
        .map_err(|error| format!("cannot read public schemas: {error}"))?
    {
        let entry = entry.map_err(|error| format!("cannot read public schema entry: {error}"))?;
        if entry.path().extension() == Some(OsStr::new("json")) {
            copy_with_mode(
                &entry.path(),
                &stage
                    .join("usr/share/pstforge/schemas")
                    .join(entry.file_name()),
                0o644,
            )?;
        }
    }
    let manpage = stage.join("usr/share/man/man1/pstforge.1");
    copy_with_mode(&root.join("docs/pstforge.1"), &manpage, 0o644)?;
    let changelog = stage.join("usr/share/doc/pstforge/changelog");
    copy_with_mode(&root.join("docs/debian-changelog"), &changelog, 0o644)?;
    run_checked_os(
        root,
        OsStr::new("gzip"),
        &[
            OsString::from("-n"),
            OsString::from("-9"),
            manpage.as_os_str().to_owned(),
        ],
    )?;
    run_checked_os(
        root,
        OsStr::new("gzip"),
        &[
            OsString::from("-n"),
            OsString::from("-9"),
            changelog.as_os_str().to_owned(),
        ],
    )?;
    let installed_size = tree_file_bytes(stage)?.div_ceil(1024);
    fs::write(
        stage.join("DEBIAN/control"),
        format!(
            "Package: pstforge\nVersion: {}\nArchitecture: amd64\nMaintainer: PSTForge Maintainers <noreply@pstforge.invalid>\nInstalled-Size: {installed_size}\nDepends: {dependencies}\nSection: utils\nPriority: optional\nHomepage: https://github.com/calculatetech/PSTForge\nDescription: read-only recovery and splitting of damaged PST files\n PSTForge reads a source PST without modifying it and writes independently\n importable Unicode PST output for recovery and checkpointed migration.\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .map_err(|error| format!("cannot write Debian control metadata: {error}"))?;
    Ok(())
}

fn validate_deb_package(
    root: &Path,
    package_root: &Path,
    package: &Path,
    version: &str,
    dependencies: &str,
) -> Result<(), String> {
    let package_text = package
        .to_str()
        .ok_or_else(|| "package path is not UTF-8".to_owned())?;
    let fields = command_stdout(root, "dpkg-deb", &["--field", package_text])?;
    for required in [
        "Package: pstforge".to_owned(),
        format!("Version: {version}"),
        "Architecture: amd64".to_owned(),
        format!("Depends: {dependencies}"),
    ] {
        if !fields.contains(&required) {
            return Err(format!("Debian metadata is missing {required}"));
        }
    }
    let contents = command_stdout(root, "dpkg-deb", &["--contents", package_text])?;
    let expected_paths = [
        "./",
        "./usr/",
        "./usr/bin/",
        "./usr/bin/pstforge",
        "./usr/share/",
        "./usr/share/doc/",
        "./usr/share/doc/pstforge/",
        "./usr/share/doc/pstforge/LICENSE-APACHE",
        "./usr/share/doc/pstforge/LICENSE-MIT",
        "./usr/share/doc/pstforge/PRODUCT_SPEC.md",
        "./usr/share/doc/pstforge/RUST-DEPENDENCY-LICENSES.txt",
        "./usr/share/man/man1/pstforge.1.gz",
        "./usr/share/doc/pstforge/README.md",
        "./usr/share/doc/pstforge/THIRD_PARTY_LICENSES.md",
        "./usr/share/doc/pstforge/WRITER-LICENSE-MIT",
        "./usr/share/doc/pstforge/changelog.gz",
        "./usr/share/doc/pstforge/copyright",
        "./usr/share/man/",
        "./usr/share/man/man1/",
        "./usr/share/pstforge/",
        "./usr/share/pstforge/schemas/",
        "./usr/share/pstforge/schemas/common.schema.json",
        "./usr/share/pstforge/schemas/info.schema.json",
        "./usr/share/pstforge/schemas/recover.schema.json",
        "./usr/share/pstforge/schemas/report.schema.json",
        "./usr/share/pstforge/schemas/split.schema.json",
        "./usr/share/pstforge/schemas/verify.schema.json",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let actual_paths = contents
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .collect::<BTreeSet<_>>();
    if actual_paths != expected_paths {
        return Err(format!(
            "Debian package path set differs from the declared set: expected {expected_paths:?}, got {actual_paths:?}"
        ));
    }
    let extracted = package_root.join("extracted");
    fs::create_dir_all(&extracted)
        .map_err(|error| format!("cannot create package extraction directory: {error}"))?;
    run_checked(
        root,
        "dpkg-deb",
        &[
            "--extract",
            package_text,
            extracted
                .to_str()
                .ok_or_else(|| "package extraction path is not UTF-8".to_owned())?,
        ],
    )?;
    let installed_binary = extracted.join("usr/bin/pstforge");
    if fs::metadata(&installed_binary)
        .map_err(|error| format!("cannot inspect packaged binary: {error}"))?
        .permissions()
        .mode()
        & 0o777
        != 0o755
    {
        return Err("packaged pstforge binary mode is not 0755".to_owned());
    }
    let reported = command_stdout_os(
        root,
        installed_binary.as_os_str(),
        &[OsString::from("--version")],
    )?;
    if reported.trim() != format!("pstforge {version}") {
        return Err(format!(
            "packaged binary reported unexpected version: {}",
            reported.trim()
        ));
    }
    let linkage = command_stdout_os(
        root,
        OsStr::new("readelf"),
        &[
            OsString::from("-d"),
            installed_binary.as_os_str().to_owned(),
        ],
    )?;
    if !linkage.contains("Shared library: [libpff.so.1]")
        || linkage.contains("(RPATH)")
        || linkage.contains("(RUNPATH)")
    {
        return Err(
            "packaged binary has unexpected libpff linkage or runtime search path".to_owned(),
        );
    }
    validate_isolated_install_remove(root, package_root, package)?;
    run_checked(root, "lintian", &["--fail-on", "error", package_text])
}

fn validate_isolated_install_remove(
    root: &Path,
    package_root: &Path,
    package: &Path,
) -> Result<(), String> {
    let install_root = package_root.join("install-root");
    let admin = install_root.join("var/lib/dpkg");
    let sentinel = install_root.join("home/operator/recovery-job/keep");
    fs::create_dir_all(&admin)
        .and_then(|()| fs::create_dir_all(sentinel.parent().unwrap_or(&install_root)))
        .map_err(|error| format!("cannot create isolated install root: {error}"))?;
    fs::write(admin.join("status"), b"")
        .and_then(|()| fs::write(&sentinel, b"operator data\n"))
        .map_err(|error| format!("cannot initialize isolated install root: {error}"))?;
    let root_argument = OsString::from(format!("--root={}", install_root.display()));
    let admin_argument = OsString::from(format!("--admindir={}", admin.display()));
    let common = [
        OsString::from("--force-not-root"),
        OsString::from("--force-depends"),
        OsString::from("--log=/dev/null"),
        root_argument,
        admin_argument,
    ];
    let mut install = common.to_vec();
    install.push(OsString::from("--install"));
    install.push(package.as_os_str().to_owned());
    run_checked_os(root, OsStr::new("dpkg"), &install)?;
    if !install_root.join("usr/bin/pstforge").is_file() {
        return Err("isolated dpkg install did not install pstforge".to_owned());
    }
    let mut remove = common.to_vec();
    remove.push(OsString::from("--remove"));
    remove.push(OsString::from("pstforge"));
    run_checked_os(root, OsStr::new("dpkg"), &remove)?;
    if install_root.join("usr/bin/pstforge").exists() || !sentinel.is_file() {
        return Err(
            "isolated dpkg removal changed operator data or retained the binary".to_owned(),
        );
    }
    Ok(())
}

fn prepare_generated_directory(root: &Path, path: &Path) -> Result<(), String> {
    let expected_parent = root.join("target");
    match fs::symlink_metadata(&expected_parent) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(format!(
                "refusing unsafe generated parent {}",
                expected_parent.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&expected_parent)
                .map_err(|error| format!("cannot create {}: {error}", expected_parent.display()))?;
        }
        Err(error) => {
            return Err(format!(
                "cannot inspect {}: {error}",
                expected_parent.display()
            ));
        }
    }
    if !path.starts_with(&expected_parent) || path == expected_parent {
        return Err(format!(
            "refusing to clean unsafe generated path {}",
            path.display()
        ));
    }
    if path.symlink_metadata().is_ok() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing non-directory generated path {}",
                path.display()
            ));
        }
        fs::remove_dir_all(path)
            .map_err(|error| format!("cannot clean {}: {error}", path.display()))?;
    }
    Ok(())
}

fn copy_with_mode(source: &Path, destination: &Path, mode: u32) -> Result<(), String> {
    fs::copy(source, destination).map_err(|error| {
        format!(
            "cannot copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    fs::set_permissions(destination, fs::Permissions::from_mode(mode)).map_err(|error| {
        format!(
            "cannot set permissions on {}: {error}",
            destination.display()
        )
    })
}

fn tree_file_bytes(root: &Path) -> Result<u64, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| format!("cannot read package entry: {error}"))?;
            let metadata = entry
                .metadata()
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| "package installed-size overflow".to_owned())?;
            }
        }
    }
    Ok(bytes)
}

fn normalize_tree_timestamps(root: &Path, epoch: &str) -> Result<(), String> {
    run_checked_os(
        root,
        OsStr::new("find"),
        &[
            root.as_os_str().to_owned(),
            OsString::from("-exec"),
            OsString::from("touch"),
            OsString::from("-h"),
            OsString::from("-d"),
            OsString::from(format!("@{epoch}")),
            OsString::from("{}"),
            OsString::from("+"),
        ],
    )
}

fn command_stdout(root: &Path, program: &str, arguments: &[&str]) -> Result<String, String> {
    command_stdout_os(
        root,
        OsStr::new(program),
        &arguments.iter().map(OsString::from).collect::<Vec<_>>(),
    )
}

fn command_stdout_os(
    root: &Path,
    program: &OsStr,
    arguments: &[OsString],
) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", program.to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed with {}: {}",
            program.to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{} returned non-UTF-8 output", program.to_string_lossy()))
}

fn run_checked(root: &Path, program: &str, arguments: &[&str]) -> Result<(), String> {
    run_checked_os(
        root,
        OsStr::new(program),
        &arguments.iter().map(OsString::from).collect::<Vec<_>>(),
    )
}

fn run_checked_os(root: &Path, program: &OsStr, arguments: &[OsString]) -> Result<(), String> {
    let status = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", program.to_string_lossy()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed with {status}",
            program.to_string_lossy()
        ))
    }
}

fn qualify_embedded_attachments(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{AttachmentContent, AttachmentSpec, FidelityStore};

    let mut fixture = FidelityStore::default();
    let embedded = fixture
        .message
        .attachments
        .iter_mut()
        .find_map(|attachment| match &mut attachment.content {
            AttachmentContent::Embedded(message) => Some(message.as_mut()),
            AttachmentContent::Binary(_)
            | AttachmentContent::Spooled(_)
            | AttachmentContent::Direct(_)
            | AttachmentContent::Reference(_)
            | AttachmentContent::Ole(_)
            | AttachmentContent::DirectOle(_) => None,
        })
        .ok_or_else(|| "writer fixture has no embedded message".to_owned())?;
    let mut nested = embedded.clone();
    nested.subject = "Nested embedded attachment checkpoint".to_owned();
    nested.attachments.clear();
    nested.attachments.push(AttachmentSpec {
        filename: "nested-payload.bin".to_owned(),
        mime_type: Some("application/octet-stream".to_owned()),
        content_id: Some("nested-payload@pstforge".to_owned()),
        content_location: Some("nested/payload.bin".to_owned()),
        rendering_position: Some(42),
        flags: 7,
        raw_properties: Vec::new(),
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Binary(b"nested payload checkpoint".to_vec()),
    });
    embedded.attachments.push(AttachmentSpec {
        filename: "embedded-payload.txt".to_owned(),
        mime_type: Some("text/plain".to_owned()),
        content_id: None,
        content_location: None,
        rendering_position: None,
        flags: 0,
        raw_properties: Vec::new(),
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Binary(b"embedded payload checkpoint".to_vec()),
    });
    embedded.attachments.push(AttachmentSpec {
        filename: "nested-message.msg".to_owned(),
        mime_type: Some("message/rfc822".to_owned()),
        content_id: None,
        content_location: None,
        rendering_position: None,
        flags: 0,
        raw_properties: Vec::new(),
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Embedded(Box::new(nested)),
    });
    publish_fidelity_qualification(root, output, &fixture, 3)
}

fn qualify_named_properties(root: &Path, output: &Path) -> Result<(), String> {
    let mut fixture = pstforge_pst::writer::FidelityStore::default();
    fixture.message.subject = "Named property fidelity checkpoint".to_owned();
    fixture.message.recipients.clear();
    fixture.message.attachments.clear();
    fixture.message.body_html = None;
    fixture.message.body_rtf = None;
    fixture.message.native_body = Some(pstforge_pst::writer::NativeBody::PlainText);
    fixture.message.rtf_in_sync = false;
    fixture.message.internet_headers = None;
    fixture.message.raw_properties.clear();
    fixture.message.spooled_properties.clear();
    fixture.message.unsupported_properties.clear();
    if fixture.message.named_properties.len() != 2 {
        return Err("writer fixture does not contain both named-property forms".to_owned());
    }
    publish_fidelity_qualification(root, output, &fixture, 1)
}

fn qualify_empty_folders(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderRole, MailFolderSpec, MailStoreSpec, MinimalStore,
    };

    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge empty folder source".to_owned(),
        folder_name: "Inbox".to_owned(),
        subject: "Empty folder fidelity checkpoint".to_owned(),
        body: "Empty folder hierarchy checkpoint.".to_owned(),
        sender_name: "PSTForge Sender".to_owned(),
        sender_email: "sender@example.com".to_owned(),
        recipient: "recipient@example.com".to_owned(),
        record_key: *b"PSTForgeEmptyFld",
    });
    fixture.message.recipients.clear();
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![
            MailFolderSpec {
                path: vec!["Deleted Items".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::DeletedItems,
                container_class: "IPF.Note".to_owned(),
                messages: Vec::new(),
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["Deleted items".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: Vec::new(),
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["Empty Parent".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: Vec::new(),
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["Empty Parent".to_owned(), "Empty Child".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: Vec::new(),
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["Inbox".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![fixture.message],
                associated_messages: Vec::new(),
            },
        ],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_contacts(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderRole, MailFolderSpec, MailStoreSpec, MinimalStore, NamedProperty,
        NamedPropertyName, NamedPropertySet, NativeBody, RawProperty, RawPropertyValue,
    };

    const PSETID_ADDRESS: [u8; 16] = [
        0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge contact source".to_owned(),
        folder_name: "Contacts".to_owned(),
        subject: "Ada Lovelace".to_owned(),
        body: "Contact notes checkpoint.".to_owned(),
        sender_name: "unused".to_owned(),
        sender_email: "unused@example.com".to_owned(),
        recipient: "unused@example.com".to_owned(),
        record_key: *b"PSTForgeContact1",
    });
    fixture.message.message_class = "IPM.Contact".to_owned();
    fixture.message.sender_name.clear();
    fixture.message.sender_email.clear();
    fixture.message.recipients.clear();
    fixture.message.native_body = Some(NativeBody::PlainText);
    fixture.message.raw_properties = vec![
        RawProperty {
            id: 0x3001,
            value: RawPropertyValue::Unicode("Ada Lovelace".to_owned()),
        },
        RawProperty {
            id: 0x3A06,
            value: RawPropertyValue::Unicode("Ada".to_owned()),
        },
        RawProperty {
            id: 0x3A11,
            value: RawPropertyValue::Unicode("Lovelace".to_owned()),
        },
        RawProperty {
            id: 0x3A08,
            value: RawPropertyValue::Unicode("+1 313 555 0100".to_owned()),
        },
        RawProperty {
            id: 0x3A1C,
            value: RawPropertyValue::Unicode("+1 313 555 0199".to_owned()),
        },
        RawProperty {
            id: 0x3A16,
            value: RawPropertyValue::Unicode("Analytical Engines Ltd".to_owned()),
        },
        RawProperty {
            id: 0x3A17,
            value: RawPropertyValue::Unicode("Programmer".to_owned()),
        },
        RawProperty {
            id: 0x3A42,
            value: RawPropertyValue::Time(130_000_000_000_000_000),
        },
    ];
    fixture.message.named_properties = [
        (0x8005, "Lovelace, Ada"),
        (0x8080, "Ada Lovelace (ada@example.com)"),
        (0x8082, "SMTP"),
        (0x8083, "ada@example.com"),
    ]
    .into_iter()
    .map(|(id, value)| NamedProperty {
        set: NamedPropertySet::Guid(PSETID_ADDRESS),
        name: NamedPropertyName::Numeric(id),
        value: RawPropertyValue::Unicode(value.to_owned()),
    })
    .collect();
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Contacts".to_owned()],
            location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Contact".to_owned(),
            messages: vec![fixture.message],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_appointments(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderRole, MailFolderSpec, MailStoreSpec, MinimalStore, NamedProperty,
        NamedPropertyName, NamedPropertySet, NativeBody, RawPropertyValue,
    };

    const PSETID_APPOINTMENT: [u8; 16] = [
        0x02, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const PSETID_COMMON: [u8; 16] = [
        0x08, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const START: i64 = 133_814_268_000_000_000;
    const END: i64 = 133_814_304_000_000_000;

    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge appointment source".to_owned(),
        folder_name: "Calendar".to_owned(),
        subject: "Appointment fidelity checkpoint".to_owned(),
        body: "Appointment notes checkpoint.".to_owned(),
        sender_name: "unused".to_owned(),
        sender_email: "unused@example.com".to_owned(),
        recipient: "unused@example.com".to_owned(),
        record_key: *b"PSTForgeAppt0001",
    });
    fixture.message.message_class = "IPM.Appointment".to_owned();
    fixture.message.sender_name.clear();
    fixture.message.sender_email.clear();
    fixture.message.recipients.clear();
    fixture.message.native_body = Some(NativeBody::PlainText);
    fixture.message.named_properties = vec![
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8205),
            value: RawPropertyValue::Integer32(2),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8208),
            value: RawPropertyValue::Unicode("Conference Room 42".to_owned()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x820D),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x820E),
            value: RawPropertyValue::Time(END),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8213),
            value: RawPropertyValue::Integer32(60),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8215),
            value: RawPropertyValue::Boolean(false),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8217),
            value: RawPropertyValue::Integer32(0),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8223),
            value: RawPropertyValue::Boolean(false),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8501),
            value: RawPropertyValue::Integer32(15),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8502),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8503),
            value: RawPropertyValue::Boolean(true),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8516),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8517),
            value: RawPropertyValue::Time(END),
        },
    ];
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Calendar".to_owned()],
            location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Appointment".to_owned(),
            messages: vec![fixture.message],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_meetings(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderRole, MailFolderSpec, MailStoreSpec, MinimalStore, NamedProperty,
        NamedPropertyName, NamedPropertySet, NativeBody, RawPropertyValue, RecipientKind,
        RecipientSpec,
    };

    const PSETID_APPOINTMENT: [u8; 16] = [
        0x02, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const PSETID_COMMON: [u8; 16] = [
        0x08, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const PSETID_MEETING: [u8; 16] = [
        0x90, 0xDA, 0xD8, 0x6E, 0x0B, 0x45, 0x1B, 0x10, 0x98, 0xDA, 0x00, 0xAA, 0x00, 0x3F, 0x13,
        0x05,
    ];
    const START: i64 = 133_814_268_000_000_000;
    const END: i64 = 133_814_304_000_000_000;
    let mut global_object_id = vec![
        0x04, 0x00, 0x00, 0x00, 0x82, 0x00, 0xE0, 0x00, 0x74, 0xC5, 0xB7, 0x10, 0x1A, 0x82, 0xE0,
        0x08, 0x00, 0x00, 0x00, 0x00,
    ];
    global_object_id.extend_from_slice(&START.to_le_bytes());
    global_object_id.extend_from_slice(&[0; 8]);
    global_object_id.extend_from_slice(&16_u32.to_le_bytes());
    global_object_id.extend_from_slice(b"PSTForgeMeeting!");
    if global_object_id.len() != 56 {
        return Err("meeting Global Object ID is not 56 bytes".to_owned());
    }

    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge meeting source".to_owned(),
        folder_name: "Inbox".to_owned(),
        subject: "Meeting request fidelity checkpoint".to_owned(),
        body: "Meeting request notes checkpoint.".to_owned(),
        sender_name: "PSTForge Organizer".to_owned(),
        sender_email: "organizer@example.com".to_owned(),
        recipient: "attendee@example.com".to_owned(),
        record_key: *b"PSTForgeMeeting1",
    });
    fixture.message.message_class = "IPM.Schedule.Meeting.Request".to_owned();
    fixture.message.native_body = Some(NativeBody::PlainText);
    fixture.message.recipients.push(RecipientSpec {
        kind: RecipientKind::Cc,
        display_name: "Optional Attendee".to_owned(),
        email_address: "optional@example.com".to_owned(),
    });
    fixture.message.named_properties = vec![
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8201),
            value: RawPropertyValue::Integer32(0),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8205),
            value: RawPropertyValue::Integer32(2),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8208),
            value: RawPropertyValue::Unicode("Conference Room 42".to_owned()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x820D),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x820E),
            value: RawPropertyValue::Time(END),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8213),
            value: RawPropertyValue::Integer32(60),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8215),
            value: RawPropertyValue::Boolean(false),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8217),
            value: RawPropertyValue::Integer32(3),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8218),
            value: RawPropertyValue::Integer32(0),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_APPOINTMENT),
            name: NamedPropertyName::Numeric(0x8223),
            value: RawPropertyValue::Boolean(false),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8501),
            value: RawPropertyValue::Integer32(15),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8502),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8503),
            value: RawPropertyValue::Boolean(true),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8516),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_COMMON),
            name: NamedPropertyName::Numeric(0x8517),
            value: RawPropertyValue::Time(END),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_MEETING),
            name: NamedPropertyName::Numeric(0x0003),
            value: RawPropertyValue::Binary(global_object_id.clone()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_MEETING),
            name: NamedPropertyName::Numeric(0x0023),
            value: RawPropertyValue::Binary(global_object_id),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_MEETING),
            name: NamedPropertyName::Numeric(0x0024),
            value: RawPropertyValue::Unicode("IPM.Appointment".to_owned()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_MEETING),
            name: NamedPropertyName::Numeric(0x0026),
            value: RawPropertyValue::Integer32(1),
        },
    ];
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Inbox".to_owned()],
            location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Note".to_owned(),
            messages: vec![fixture.message],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_pim_items(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderRole, MailFolderSpec, MailStoreSpec, MinimalStore, NamedProperty,
        NamedPropertyName, NamedPropertySet, NativeBody, RawPropertyValue,
    };

    const PSETID_TASK: [u8; 16] = [
        0x03, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const PSETID_NOTE: [u8; 16] = [
        0x0E, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const START: i64 = 133_815_132_000_000_000;
    const DUE: i64 = 133_819_452_000_000_000;

    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge PIM source".to_owned(),
        folder_name: "Tasks".to_owned(),
        subject: "Task fidelity checkpoint".to_owned(),
        body: "Task notes checkpoint.".to_owned(),
        sender_name: "unused".to_owned(),
        sender_email: "unused@example.com".to_owned(),
        recipient: "unused@example.com".to_owned(),
        record_key: *b"PSTForgePimItems",
    });
    fixture.message.message_class = "IPM.Task".to_owned();
    fixture.message.sender_name.clear();
    fixture.message.sender_email.clear();
    fixture.message.recipients.clear();
    fixture.message.native_body = Some(NativeBody::PlainText);
    fixture.message.named_properties = vec![
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_TASK),
            name: NamedPropertyName::Numeric(0x8101),
            value: RawPropertyValue::Integer32(0),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_TASK),
            name: NamedPropertyName::Numeric(0x8102),
            value: RawPropertyValue::Floating64(0_f64.to_bits()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_TASK),
            name: NamedPropertyName::Numeric(0x8104),
            value: RawPropertyValue::Time(START),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_TASK),
            name: NamedPropertyName::Numeric(0x8105),
            value: RawPropertyValue::Time(DUE),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_TASK),
            name: NamedPropertyName::Numeric(0x811C),
            value: RawPropertyValue::Boolean(false),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_TASK),
            name: NamedPropertyName::Numeric(0x8126),
            value: RawPropertyValue::Boolean(false),
        },
    ];

    let mut sticky_note = fixture.message.clone();
    sticky_note.message_class = "IPM.StickyNote".to_owned();
    sticky_note.subject = "Sticky note fidelity checkpoint".to_owned();
    sticky_note.named_properties = vec![
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_NOTE),
            name: NamedPropertyName::Numeric(0x8B00),
            value: RawPropertyValue::Integer32(3),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_NOTE),
            name: NamedPropertyName::Numeric(0x8B02),
            value: RawPropertyValue::Integer32(320),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_NOTE),
            name: NamedPropertyName::Numeric(0x8B03),
            value: RawPropertyValue::Integer32(240),
        },
    ];

    let mut post = fixture.message.clone();
    post.message_class = "IPM.Post".to_owned();
    post.subject = "Post fidelity checkpoint".to_owned();
    post.sender_name = "PSTForge Poster".to_owned();
    post.sender_email = "poster@example.com".to_owned();
    post.named_properties.clear();

    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![
            MailFolderSpec {
                path: vec!["Tasks".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Task".to_owned(),
                messages: vec![fixture.message],
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["Notes".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.StickyNote".to_owned(),
                messages: vec![sticky_note],
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["Posts".to_owned()],
                location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![post],
                associated_messages: Vec::new(),
            },
        ],
    };
    publish_qualification(root, output, 3, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_distribution_list(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderRole, MailFolderSpec, MailStoreSpec, NamedProperty,
        NamedPropertyName, NamedPropertySet, RawProperty, RawPropertyValue,
    };

    const PSETID_ADDRESS: [u8; 16] = [
        0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const ONE_OFF_PROVIDER: [u8; 16] = [
        0x81, 0x2B, 0x1F, 0xA4, 0xBE, 0xA3, 0x10, 0x19, 0x9D, 0x6E, 0x00, 0xDD, 0x01, 0x0F, 0x54,
        0x02,
    ];
    let one_off = |display_name: &str, address: &str| {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&ONE_OFF_PROVIDER);
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(display_name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(b"SMTP\0");
        bytes.extend_from_slice(address.as_bytes());
        bytes.push(0);
        bytes
    };

    let list_name = "PSTForge distribution list checkpoint";
    let members = vec![
        one_off("Ada Example", "ada@example.com"),
        one_off("Grace Example", "grace@example.com"),
    ];
    let mut fixture = FidelityStore {
        store_name: "PSTForge distribution list source".to_owned(),
        record_key: *b"PSTForgeDLSource",
        ..FidelityStore::default()
    };
    fixture.message.message_class = "IPM.DistList".to_owned();
    fixture.message.subject = list_name.to_owned();
    fixture.message.sender_name.clear();
    fixture.message.sender_email.clear();
    fixture.message.recipients.clear();
    fixture.message.body_text = None;
    fixture.message.body_html = None;
    fixture.message.body_rtf = None;
    fixture.message.native_body = None;
    fixture.message.attachments.clear();
    fixture.message.raw_properties = vec![RawProperty {
        id: 0x3001,
        value: RawPropertyValue::Unicode(list_name.to_owned()),
    }];
    fixture.message.named_properties = vec![
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_ADDRESS),
            name: NamedPropertyName::Numeric(0x8053),
            value: RawPropertyValue::Unicode(list_name.to_owned()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_ADDRESS),
            name: NamedPropertyName::Numeric(0x8055),
            value: RawPropertyValue::MultipleBinary(members.clone()),
        },
        NamedProperty {
            set: NamedPropertySet::Guid(PSETID_ADDRESS),
            name: NamedPropertyName::Numeric(0x8054),
            value: RawPropertyValue::MultipleBinary(members),
        },
    ];
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Contacts".to_owned()],
            location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Contact".to_owned(),
            messages: vec![fixture.message],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_document_object(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        AttachmentContent, AttachmentSpec, FidelityStore, MailFolderLocation, MailFolderRole,
        MailFolderSpec, MailStoreSpec, MinimalStore, NamedProperty, NamedPropertyName,
        NamedPropertySet, RawProperty, RawPropertyValue,
    };

    let document = document_checkpoint_payload()?;
    let display_name = "PSTForge document checkpoint.docx";
    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge Document source".to_owned(),
        folder_name: "Documents".to_owned(),
        subject: display_name.to_owned(),
        body: String::new(),
        sender_name: String::new(),
        sender_email: String::new(),
        recipient: String::new(),
        record_key: *b"PSTForgeDocument",
    });
    fixture.message.message_class = "IPM.Document.Word.Document.12".to_owned();
    fixture.message.sender_name.clear();
    fixture.message.sender_email.clear();
    fixture.message.recipients.clear();
    fixture.message.body_text = None;
    fixture.message.body_html = None;
    fixture.message.body_rtf = None;
    fixture.message.native_body = None;
    fixture.message.raw_properties = vec![RawProperty {
        id: 0x3001,
        value: RawPropertyValue::Unicode(display_name.to_owned()),
    }];
    fixture.message.named_properties = vec![
        NamedProperty {
            set: NamedPropertySet::PublicStrings,
            name: NamedPropertyName::String("Title".to_owned()),
            value: RawPropertyValue::Unicode("PSTForge document checkpoint".to_owned()),
        },
        NamedProperty {
            set: NamedPropertySet::PublicStrings,
            name: NamedPropertyName::String("Keywords".to_owned()),
            value: RawPropertyValue::MultipleUnicode(vec![
                "recovery".to_owned(),
                "checkpoint".to_owned(),
                "Unicode \u{4E16}\u{754C}".to_owned(),
            ]),
        },
        NamedProperty {
            set: NamedPropertySet::PublicStrings,
            name: NamedPropertyName::String("Comments".to_owned()),
            value: RawPropertyValue::Unicode("Document metadata checkpoint".to_owned()),
        },
        NamedProperty {
            set: NamedPropertySet::PublicStrings,
            name: NamedPropertyName::String("PageCount".to_owned()),
            value: RawPropertyValue::Integer32(1),
        },
    ];
    fixture.message.attachments = vec![AttachmentSpec {
        filename: display_name.to_owned(),
        mime_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_owned(),
        ),
        content_id: None,
        content_location: None,
        rendering_position: Some(-1),
        flags: 0,
        raw_properties: Vec::new(),
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Binary(document),
    }];
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Documents".to_owned()],
            location: MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Note".to_owned(),
            messages: vec![fixture.message],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn document_checkpoint_payload() -> Result<Vec<u8>, String> {
    use std::io::{Cursor, Write as _};

    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive
        .start_file("[Content_Types].xml", options)
        .map_err(|error| format!("cannot start DOCX content-types part: {error}"))?;
    archive
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        )
        .map_err(|error| format!("cannot write DOCX content-types part: {error}"))?;
    archive
        .start_file("_rels/.rels", options)
        .map_err(|error| format!("cannot start DOCX package relationships: {error}"))?;
    archive
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        )
        .map_err(|error| format!("cannot write DOCX package relationships: {error}"))?;
    archive
        .start_file("word/document.xml", options)
        .map_err(|error| format!("cannot start DOCX document part: {error}"))?;
    archive
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>PSTForge document checkpoint</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#,
        )
        .map_err(|error| format!("cannot write DOCX document part: {error}"))?;
    archive
        .finish()
        .map_err(|error| format!("cannot finish DOCX fixture: {error}"))
        .map(std::io::Cursor::into_inner)
}

fn qualify_reference_attachments(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        AttachmentContent, AttachmentReferenceMethod, AttachmentReferenceSpec, AttachmentSpec,
        FidelityStore, MailFolderLocation, MailFolderRole, MailFolderSpec, MailStoreSpec,
        MinimalStore,
    };

    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge reference source".to_owned(),
        folder_name: "Reference Attachments".to_owned(),
        subject: "Reference attachment fidelity checkpoint".to_owned(),
        body: "Data-less MAPI reference attachment checkpoint.".to_owned(),
        sender_name: "PSTForge".to_owned(),
        sender_email: "sender@example.com".to_owned(),
        recipient: "recipient@example.com".to_owned(),
        record_key: *b"PSTForgeRefAtt01",
    });
    let reference = |method, filename: &str, long_pathname: &str| AttachmentSpec {
        filename: filename.to_owned(),
        mime_type: None,
        content_id: None,
        content_location: None,
        rendering_position: Some(-1),
        flags: 0,
        raw_properties: Vec::new(),
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Reference(AttachmentReferenceSpec {
            method,
            long_pathname: long_pathname.to_owned(),
            pathname: Some(filename.to_owned()),
            provider_type: None,
            original_permission: None,
            permission: None,
        }),
    };
    fixture.message.attachments = vec![
        reference(
            AttachmentReferenceMethod::ByReference,
            "shared-reference.txt",
            r"\\unreachable.invalid\recovery\shared-reference.txt",
        ),
        reference(
            AttachmentReferenceMethod::ByReferenceResolve,
            "resolved-reference.txt",
            r"\\unreachable.invalid\recovery\resolved-reference.txt",
        ),
        reference(
            AttachmentReferenceMethod::ByReferenceOnly,
            "reference-only.txt",
            r"Z:\unavailable\reference-only.txt",
        ),
        AttachmentSpec {
            filename: "web-reference.docx".to_owned(),
            mime_type: Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .to_owned(),
            ),
            content_id: None,
            content_location: None,
            rendering_position: Some(-1),
            flags: 0,
            raw_properties: Vec::new(),
            spooled_properties: Vec::new(),
            direct_properties: Vec::new(),
            content: AttachmentContent::Reference(AttachmentReferenceSpec {
                method: AttachmentReferenceMethod::ByWebReference,
                long_pathname: "https://example.invalid/recovery/web-reference.docx".to_owned(),
                pathname: None,
                provider_type: Some("RecoveryProvider".to_owned()),
                original_permission: Some(1),
                permission: Some(2),
            }),
        },
    ];
    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Reference Attachments".to_owned()],
            location: MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Note".to_owned(),
            messages: vec![fixture.message],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 1, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_ole_attachments(root: &Path, output: &Path) -> Result<(), String> {
    use std::io::{Cursor, Write as _};

    use pstforge_pst::writer::{
        AttachmentContent, AttachmentSpec, FidelityStore, FileBlobSpec, MinimalStore,
        OleAttachmentSpec, OleDataKind, RawProperty, RawPropertyValue, SpooledPropertySpec,
    };

    let parent = output
        .parent()
        .filter(|path| path.is_dir())
        .ok_or_else(|| "qualification output parent must already exist".to_owned())?;
    let payloads = tempfile::tempdir_in(parent)
        .map_err(|error| format!("cannot create OLE payload staging directory: {error}"))?;
    let mut compound =
        cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new()))
            .map_err(|error| format!("cannot create OLE2 compound file: {error}"))?;
    compound
        .create_storage("/PSTForge")
        .map_err(|error| format!("cannot create OLE2 storage: {error}"))?;
    compound
        .create_stream("/PSTForge/Contents")
        .and_then(|mut stream| stream.write_all(b"PSTForge OLE2 recovery checkpoint"))
        .map_err(|error| format!("cannot create OLE2 content stream: {error}"))?;
    let ole2 = compound.into_inner().into_inner();
    cfb::CompoundFile::open_strict(Cursor::new(ole2.clone()))
        .map_err(|error| format!("generated OLE2 fixture is invalid: {error}"))?;
    let ole1 = b"PSTForge OLE1 native recovery checkpoint".to_vec();
    let rendition = vec![0x5A; 20 * 1024];
    let spool = |name: &str, bytes: &[u8]| -> Result<FileBlobSpec, String> {
        let path = payloads.path().join(name);
        fs::write(&path, bytes)
            .map_err(|error| format!("cannot write OLE fixture payload: {error}"))?;
        Ok(FileBlobSpec {
            path,
            offset: 0,
            byte_len: u64::try_from(bytes.len())
                .map_err(|_| "OLE fixture payload length is out of range".to_owned())?,
            sha256: Sha256::digest(bytes).into(),
        })
    };
    let attachment = |filename: &str,
                      data: FileBlobSpec,
                      data_kind: OleDataKind,
                      raw_properties: Vec<RawProperty>| AttachmentSpec {
        filename: filename.to_owned(),
        mime_type: None,
        content_id: None,
        content_location: None,
        rendering_position: Some(-1),
        flags: 0,
        raw_properties,
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Ole(OleAttachmentSpec { data, data_kind }),
    };

    let mut fixture = FidelityStore::from(&MinimalStore {
        store_name: "PSTForge OLE source".to_owned(),
        folder_name: "OLE Attachments".to_owned(),
        subject: "OLE attachment fidelity checkpoint".to_owned(),
        body: "Both documented ATTACH_OLE payload representations are present.".to_owned(),
        sender_name: "PSTForge".to_owned(),
        sender_email: "sender@example.com".to_owned(),
        recipient: "recipient@example.com".to_owned(),
        record_key: *b"PSTForgeOleAtt01",
    });
    fixture.message.attachments = vec![
        attachment(
            "ole2-storage.bin",
            spool("ole2-storage.bin", &ole2)?,
            OleDataKind::Object,
            vec![
                RawProperty {
                    id: 0x3702,
                    value: RawPropertyValue::Binary(vec![0x01, 0x00]),
                },
                RawProperty {
                    id: 0x370A,
                    value: RawPropertyValue::Binary(vec![
                        0x2A, 0x86, 0x48, 0x86, 0xF7, 0x14, 0x03, 0x0A, 0x03, 0x02, 0x01,
                    ]),
                },
            ],
        ),
        attachment(
            "ole1-native.bin",
            spool("ole1-native.bin", &ole1)?,
            OleDataKind::Binary,
            vec![RawProperty {
                id: 0x3709,
                value: RawPropertyValue::Binary(Vec::new()),
            }],
        ),
    ];
    fixture.message.attachments[0]
        .spooled_properties
        .push(SpooledPropertySpec {
            id: 0x3709,
            property_type: 0x0102,
            blob: spool("ole2-rendition.wmf", &rendition)?,
        });
    publish_fidelity_qualification(root, output, &fixture, 1)
}

fn qualify_outlook_ole_objects(root: &Path, output: &Path) -> Result<(), String> {
    use std::io::{Cursor, Write as _};

    use pstforge_pst::writer::{
        AttachmentContent, AttachmentSpec, FidelityStore, FileBlobSpec, OleAttachmentSpec,
        OleDataKind, RawProperty, RawPropertyValue,
    };

    let parent = output
        .parent()
        .filter(|path| path.is_dir())
        .ok_or_else(|| "qualification output parent must already exist".to_owned())?;
    let payloads = tempfile::tempdir_in(parent)
        .map_err(|error| format!("cannot create Outlook OLE staging directory: {error}"))?;
    let mut compound =
        cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new()))
            .map_err(|error| format!("cannot create Outlook OLE compound file: {error}"))?;
    compound
        .create_storage("/PSTForge")
        .map_err(|error| format!("cannot create Outlook OLE storage: {error}"))?;
    compound
        .create_stream("/PSTForge/Contents")
        .and_then(|mut stream| stream.write_all(b"PSTForge Outlook OLE object checkpoint"))
        .map_err(|error| format!("cannot create Outlook OLE content stream: {error}"))?;
    let payload = compound.into_inner().into_inner();
    cfb::CompoundFile::open_strict(Cursor::new(payload.clone()))
        .map_err(|error| format!("generated Outlook OLE fixture is invalid: {error}"))?;

    let mut fixture = FidelityStore {
        store_name: "PSTForge Outlook OLE source".to_owned(),
        folder_name: "OLE Objects".to_owned(),
        ..FidelityStore::default()
    };
    fixture.message.subject = "Outlook OLE object fidelity checkpoint".to_owned();
    fixture.message.attachments.clear();
    for index in 0_u8..5 {
        let path = payloads.path().join(format!("object-{index}.bin"));
        fs::write(&path, &payload)
            .map_err(|error| format!("cannot write Outlook OLE payload: {error}"))?;
        fixture.message.attachments.push(AttachmentSpec {
            filename: format!("object-{index}.bin"),
            mime_type: None,
            content_id: None,
            content_location: None,
            rendering_position: Some(-1),
            flags: 0,
            raw_properties: vec![
                RawProperty {
                    id: 0x3702,
                    value: RawPropertyValue::Binary(vec![0x01, index]),
                },
                RawProperty {
                    id: 0x370A,
                    value: RawPropertyValue::Binary(vec![0x2A, 0x86, 0x48, index]),
                },
            ],
            spooled_properties: Vec::new(),
            direct_properties: Vec::new(),
            content: AttachmentContent::Ole(OleAttachmentSpec {
                data: FileBlobSpec {
                    path,
                    offset: 0,
                    byte_len: u64::try_from(payload.len())
                        .map_err(|_| "Outlook OLE payload length is out of range".to_owned())?,
                    sha256: Sha256::digest(&payload).into(),
                },
                data_kind: OleDataKind::Object,
            }),
        });
    }
    publish_fidelity_qualification(root, output, &fixture, 1)
}

fn qualify_calendar_exceptions(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        AttachmentContent, AttachmentSpec, FidelityStore, MailFolderRole, MailFolderSpec,
        MailStoreSpec, RawProperty, RawPropertyValue,
    };

    const ORIGINAL_START: i64 = 133_815_132_000_000_000;
    const ORIGINAL_END: i64 = 133_815_168_000_000_000;

    let fixture = FidelityStore::default();
    let mut appointment = fixture.message;
    appointment.message_class = "IPM.Appointment".to_owned();
    appointment.subject = "Recurring appointment exception checkpoint".to_owned();
    appointment.recipients.clear();
    appointment.attachments.clear();
    appointment.raw_properties.clear();

    let mut exception = appointment.clone();
    exception.message_class = "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}".to_owned();
    exception.subject = "Modified recurrence instance checkpoint".to_owned();
    exception.attachments.clear();

    appointment.attachments.push(AttachmentSpec {
        filename: "Modified recurrence instance.msg".to_owned(),
        mime_type: Some("application/vnd.ms-outlook".to_owned()),
        content_id: None,
        content_location: None,
        rendering_position: None,
        flags: 0,
        raw_properties: vec![
            RawProperty {
                id: 0x3001,
                value: RawPropertyValue::Unicode(
                    "Modified recurrence instance checkpoint".to_owned(),
                ),
            },
            RawProperty {
                id: 0x3702,
                value: RawPropertyValue::Binary(Vec::new()),
            },
            RawProperty {
                id: 0x3709,
                value: RawPropertyValue::Binary(vec![0x01, 0x00, 0x00, 0x00]),
            },
            RawProperty {
                id: 0x7FFA,
                value: RawPropertyValue::Integer32(0),
            },
            RawProperty {
                id: 0x7FFB,
                value: RawPropertyValue::Time(ORIGINAL_START),
            },
            RawProperty {
                id: 0x7FFC,
                value: RawPropertyValue::Time(ORIGINAL_END),
            },
            RawProperty {
                id: 0x7FFD,
                value: RawPropertyValue::Integer32(2),
            },
            RawProperty {
                id: 0x7FFE,
                value: RawPropertyValue::Boolean(true),
            },
            RawProperty {
                id: 0x7FFF,
                value: RawPropertyValue::Boolean(false),
            },
        ],
        spooled_properties: Vec::new(),
        direct_properties: Vec::new(),
        content: AttachmentContent::Embedded(Box::new(exception)),
    });

    let spec = MailStoreSpec {
        store_name: fixture.store_name,
        record_key: fixture.record_key,
        folders: vec![MailFolderSpec {
            path: vec!["Calendar".to_owned()],
            location: pstforge_pst::writer::MailFolderLocation::IpmSubtree,
            role: MailFolderRole::Ordinary,
            container_class: "IPF.Appointment".to_owned(),
            messages: vec![appointment],
            associated_messages: Vec::new(),
        }],
    };
    publish_qualification(root, output, 2, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn qualify_associated_data(root: &Path, output: &Path) -> Result<(), String> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderLocation, MailFolderRole, MailFolderSpec, MailStoreSpec,
        RawProperty, RawPropertyValue,
    };

    let fixture = FidelityStore::default();
    let mut normal = fixture.message;
    normal.message_class = "IPM.Microsoft.SniffData".to_owned();
    normal.subject = "Root configuration checkpoint".to_owned();
    normal.sender_name.clear();
    normal.sender_email.clear();
    normal.recipients.clear();
    normal.attachments.clear();
    normal.body_text = None;
    normal.body_html = None;
    normal.body_rtf = None;
    normal.native_body = None;
    normal.rtf_in_sync = false;
    normal.internet_headers = None;
    normal.named_properties.clear();
    normal.spooled_properties.clear();
    normal.unsupported_properties.clear();
    normal.raw_properties = vec![
        RawProperty {
            id: 0x6000,
            value: RawPropertyValue::Integer32(42),
        },
        RawProperty {
            id: 0x6001,
            value: RawPropertyValue::Unicode("root property checkpoint".to_owned()),
        },
        RawProperty {
            id: 0x6002,
            value: RawPropertyValue::Binary(vec![0, 1, 2, 3, 0xFE, 0xFF]),
        },
    ];
    let mut associated = normal.clone();
    associated.message_class = "IPM.Configuration.PSTForge".to_owned();
    associated.subject = "Subject fallback must not replace display name".to_owned();
    associated.raw_properties[0].value = RawPropertyValue::Integer32(84);
    associated.raw_properties[1].value =
        RawPropertyValue::Unicode("associated property checkpoint".to_owned());
    associated.raw_properties.push(RawProperty {
        id: 0x3001,
        value: RawPropertyValue::Unicode("Associated configuration checkpoint".to_owned()),
    });
    let spec = MailStoreSpec {
        store_name: "PSTForge root and associated data".to_owned(),
        record_key: *b"PSTForgePlace001",
        folders: vec![
            MailFolderSpec {
                path: vec!["Freebusy Data".to_owned()],
                location: MailFolderLocation::StoreRoot,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![normal],
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["IPM_COMMON_VIEWS".to_owned()],
                location: MailFolderLocation::StoreRoot,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: Vec::new(),
                associated_messages: vec![associated],
            },
        ],
    };
    publish_qualification(root, output, 2, |part| {
        pstforge_pst::writer::create_mail_store(part, &spec)
    })
}

fn publish_fidelity_qualification(
    root: &Path,
    output: &Path,
    fixture: &pstforge_pst::writer::FidelityStore,
    item_count: u64,
) -> Result<(), String> {
    publish_qualification(root, output, item_count, |part| {
        pstforge_pst::writer::create_fidelity_store(part, fixture)
    })
}

fn publish_qualification(
    root: &Path,
    output: &Path,
    item_count: u64,
    create: impl FnOnce(
        &Path,
    ) -> Result<
        pstforge_pst::writer::FidelityWriteReport,
        pstforge_pst::writer::WriterError,
    >,
) -> Result<(), String> {
    if !output.is_absolute() || output.starts_with(root) || output.exists() {
        return Err(
            "qualification output must be a new absolute directory outside the repository"
                .to_owned(),
        );
    }
    let parent = output
        .parent()
        .filter(|path| path.is_dir())
        .ok_or_else(|| "qualification output parent must already exist".to_owned())?;
    let temporary = tempfile::Builder::new()
        .prefix(".pstforge-0.5.0-")
        .tempdir_in(parent)
        .map_err(|error| format!("cannot create qualification staging directory: {error}"))?;
    let parts = temporary.path().join("parts");
    fs::create_dir(&parts)
        .map_err(|error| format!("cannot create qualification parts directory: {error}"))?;
    fs::set_permissions(&parts, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot secure qualification parts directory: {error}"))?;
    let part = parts.join("part-0001.pst");
    let report =
        create(&part).map_err(|error| format!("cannot create qualification PST: {error}"))?;
    if !report.unsupported_properties.is_empty() {
        return Err("qualification PST omitted writer properties".to_owned());
    }

    let mut file = fs::File::open(&part)
        .map_err(|error| format!("cannot reopen qualification PST: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash qualification PST: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let byte_len = file
        .metadata()
        .map_err(|error| format!("cannot inspect qualification PST: {error}"))?
        .len();
    let log = format!(
        "PSTForge recovery log\nVersion: {}\nResult: complete\n\nRecovery summary\nItems written: {item_count}\nOutput files: 1\n\nData not copied\nNo readable data was skipped.\n\nOutput files\npart-0001.pst: {byte_len} bytes, SHA-256 {:x}\n",
        env!("CARGO_PKG_VERSION"),
        hasher.finalize()
    );
    let log_path = temporary.path().join("recovery.log");
    fs::write(&log_path, log)
        .map_err(|error| format!("cannot write qualification recovery log: {error}"))?;
    fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot secure qualification recovery log: {error}"))?;
    fs::rename(temporary.path(), output)
        .map_err(|error| format!("cannot publish qualification directory: {error}"))?;
    let _published = temporary.keep();
    println!(
        "qualification PST: {}",
        output.join("parts/part-0001.pst").display()
    );
    Ok(())
}

impl Gate {
    fn fast(&self) -> Result<(), String> {
        self.command("format", "cargo", &["fmt", "--all", "--", "--check"])?;
        self.command(
            "check",
            "cargo",
            &["check", "--workspace", "--all-targets", "--locked"],
        )?;
        self.command(
            "clippy",
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        )?;
        self.command(
            "tests",
            "cargo",
            &["test", "--workspace", "--all-targets", "--locked"],
        )?;
        self.documentation()?;
        self.validate_documents_and_schemas()?;
        self.command("diff-check", "git", &["diff", "--check"])?;
        println!("fast gate passed; evidence: {}", self.evidence.display());
        Ok(())
    }

    fn full(&self) -> Result<(), String> {
        self.fast()?;
        self.validate_licenses()?;
        self.command("advisories", "cargo", &["audit", "--deny", "warnings"])?;
        self.validate_generated_store()?;
        let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST").ok_or_else(|| {
            "PSTFORGE_CORPUS_MANIFEST must point to an external corpus manifest for the full gate"
                .to_owned()
        })?;
        let manifest = self.load_manifest(Path::new(&manifest_path))?;
        let prepared = self.prepare_full_corpus(manifest)?;
        self.command_with_env(
            "external-corpus",
            "cargo",
            &[
                "test",
                "-p",
                "pstforge-cli",
                "--test",
                "external_corpus",
                "--release",
                "--locked",
                "--",
                "--ignored",
                "--nocapture",
            ],
            "PSTFORGE_CORPUS_MANIFEST",
            prepared.manifest_path.as_os_str(),
        )?;
        self.run_independent_readers(&prepared.manifest)?;
        println!("full gate passed; evidence: {}", self.evidence.display());
        Ok(())
    }

    fn release(&self) -> Result<(), String> {
        self.full()?;
        self.command(
            "release-build",
            "cargo",
            &["build", "--workspace", "--release", "--locked"],
        )?;
        println!(
            "release gate foundation passed; evidence: {}",
            self.evidence.display()
        );
        Ok(())
    }

    fn command(&self, name: &str, program: &str, args: &[&str]) -> Result<(), String> {
        print!("{name} ... ");
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .output()
            .map_err(|error| format!("cannot run {program}: {error}"))?;
        self.record(name, program, args, &output)?;
        if output.status.success() {
            println!("ok");
            Ok(())
        } else {
            println!("FAILED");
            Err(format!(
                "{name} failed with {}; see {}",
                output.status,
                self.evidence.join(format!("{name}.log")).display()
            ))
        }
    }

    fn command_with_env(
        &self,
        name: &str,
        program: &str,
        args: &[&str],
        key: &str,
        value: &OsStr,
    ) -> Result<(), String> {
        print!("{name} ... ");
        let output = Command::new(program)
            .args(args)
            .env(key, value)
            .current_dir(&self.root)
            .output()
            .map_err(|error| format!("cannot run {program}: {error}"))?;
        self.record(name, program, args, &output)?;
        if output.status.success() {
            println!("ok");
            Ok(())
        } else {
            println!("FAILED");
            Err(format!(
                "{name} failed with {}; see {}",
                output.status,
                self.evidence.join(format!("{name}.log")).display()
            ))
        }
    }

    fn record(
        &self,
        name: &str,
        program: &str,
        args: &[&str],
        output: &Output,
    ) -> Result<(), String> {
        let mut content = format!(
            "command: {program} {}\nstatus: {}\n\nstdout:\n",
            args.join(" "),
            output.status
        );
        content.push_str(&String::from_utf8_lossy(&output.stdout));
        content.push_str("\n\nstderr:\n");
        content.push_str(&String::from_utf8_lossy(&output.stderr));
        fs::write(self.evidence.join(format!("{name}.log")), content)
            .map_err(|error| format!("cannot record {name} evidence: {error}"))
    }

    fn documentation(&self) -> Result<(), String> {
        let name = "documentation";
        let args = [
            "doc",
            "--workspace",
            "--no-deps",
            "--locked",
            "--document-private-items",
        ];
        print!("{name} ... ");
        let output = Command::new("cargo")
            .args(args)
            .env("RUSTDOCFLAGS", "-D warnings")
            .current_dir(&self.root)
            .output()
            .map_err(|error| format!("cannot run cargo doc: {error}"))?;
        self.record(name, "cargo", &args, &output)?;
        if output.status.success() {
            println!("ok");
            Ok(())
        } else {
            println!("FAILED");
            Err(format!(
                "documentation failed with {}; see {}",
                output.status,
                self.evidence.join("documentation.log").display()
            ))
        }
    }

    fn validate_documents_and_schemas(&self) -> Result<(), String> {
        for relative in [
            "AGENTS.md",
            "README.md",
            "THIRD_PARTY_LICENSES.md",
            ".agent/EXECPLAN.md",
            ".agent/PLANS.md",
            "docs/PRODUCT_SPEC.md",
            "docs/ROADMAP.md",
            "docs/debian-changelog",
            "docs/debian-copyright",
            "docs/pstforge.1",
            "docs/schemas/common.schema.json",
            "docs/schemas/info.schema.json",
            "docs/schemas/verify.schema.json",
            "docs/schemas/recover.schema.json",
            "docs/schemas/split.schema.json",
            "docs/schemas/report.schema.json",
            "tests/corpus-schema.json",
            "tests/corpus-manifest.example.toml",
            "tests/fixtures/json/info.json",
            "tests/fixtures/json/verify.json",
            "tests/fixtures/json/recover.json",
            "tests/fixtures/json/split.json",
            "tests/fixtures/json/report.json",
        ] {
            if !self.root.join(relative).is_file() {
                return Err(format!(
                    "required documentation artifact is missing: {relative}"
                ));
            }
        }
        let schema = fs::read_to_string(self.root.join("tests/corpus-schema.json"))
            .map_err(|error| format!("cannot read corpus schema: {error}"))?;
        serde_json::from_str::<serde_json::Value>(&schema)
            .map_err(|error| format!("corpus schema is not valid JSON: {error}"))?;
        for relative in [
            "docs/schemas/common.schema.json",
            "docs/schemas/info.schema.json",
            "docs/schemas/verify.schema.json",
            "docs/schemas/recover.schema.json",
            "docs/schemas/split.schema.json",
            "docs/schemas/report.schema.json",
            "tests/fixtures/json/info.json",
            "tests/fixtures/json/verify.json",
            "tests/fixtures/json/recover.json",
            "tests/fixtures/json/split.json",
            "tests/fixtures/json/report.json",
        ] {
            let document = fs::read_to_string(self.root.join(relative))
                .map_err(|error| format!("cannot read {relative}: {error}"))?;
            serde_json::from_str::<serde_json::Value>(&document)
                .map_err(|error| format!("{relative} is not valid JSON: {error}"))?;
        }
        self.validate_public_json_schemas()?;
        let example = fs::read_to_string(self.root.join("tests/corpus-manifest.example.toml"))
            .map_err(|error| format!("cannot read example manifest: {error}"))?;
        let example: CorpusManifest = toml::from_str(&example)
            .map_err(|error| format!("example manifest is not valid TOML: {error}"))?;
        self.validate_manifest(&example)?;
        fs::write(
            self.evidence.join("artifacts.log"),
            "documentation and schema syntax: ok\n",
        )
        .map_err(|error| format!("cannot record artifact validation: {error}"))?;
        println!("artifacts ... ok");
        Ok(())
    }

    fn validate_public_json_schemas(&self) -> Result<(), String> {
        const SCHEMAS: &[&str] = &[
            "common.schema.json",
            "info.schema.json",
            "verify.schema.json",
            "recover.schema.json",
            "split.schema.json",
            "report.schema.json",
        ];
        const COMMANDS: &[(&str, &str)] = &[
            ("info.schema.json", "info.json"),
            ("verify.schema.json", "verify.json"),
            ("recover.schema.json", "recover.json"),
            ("split.schema.json", "split.json"),
            ("report.schema.json", "report.json"),
        ];

        let mut schemas = Vec::with_capacity(SCHEMAS.len());
        let mut registry = jsonschema::Registry::new();
        for filename in SCHEMAS {
            let path = self.root.join("docs/schemas").join(filename);
            let value: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(&path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
            )
            .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
            let uri = format!("file:///pstforge-schemas/{filename}");
            registry = registry
                .add(uri, value.clone())
                .map_err(|error| format!("cannot register {filename}: {error}"))?;
            schemas.push((*filename, value));
        }
        let registry = registry
            .prepare()
            .map_err(|error| format!("cannot prepare public schema registry: {error}"))?;

        for (schema_filename, fixture_filename) in COMMANDS {
            let schema = schemas
                .iter()
                .find_map(|(filename, value)| (*filename == *schema_filename).then_some(value))
                .ok_or_else(|| format!("public schema {schema_filename} was not loaded"))?;
            let base_uri = format!("file:///pstforge-schemas/{schema_filename}");
            let validator = jsonschema::options()
                .with_registry(&registry)
                .with_base_uri(&base_uri)
                .build(schema)
                .map_err(|error| format!("cannot compile {schema_filename}: {error}"))?;
            let fixture_path = self.root.join("tests/fixtures/json").join(fixture_filename);
            let fixture: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(&fixture_path)
                    .map_err(|error| format!("cannot read {}: {error}", fixture_path.display()))?,
            )
            .map_err(|error| format!("{} is not valid JSON: {error}", fixture_path.display()))?;
            if let Err(error) = validator.validate(&fixture) {
                return Err(format!(
                    "{fixture_filename} violates {schema_filename}: {error}"
                ));
            }
        }
        Ok(())
    }

    fn validate_licenses(&self) -> Result<(), String> {
        print!("licenses ... ");
        let output = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--locked"])
            .current_dir(&self.root)
            .output()
            .map_err(|error| format!("cannot inspect Cargo licenses: {error}"))?;
        if !output.status.success() {
            println!("FAILED");
            return Err(format!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("cannot parse cargo metadata: {error}"))?;
        let mut rejected = Vec::new();
        for package in &metadata.packages {
            let Some(license) = &package.license else {
                rejected.push(format!("{} {}: missing", package.name, package.version));
                continue;
            };
            let has_permissive_choice = [
                "MIT",
                "Apache-2.0",
                "BSD-2-Clause",
                "BSD-3-Clause",
                "ISC",
                "Zlib",
                "Unicode-3.0",
                "Unlicense",
                "CC0-1.0",
                "MPL-2.0",
            ]
            .iter()
            .any(|allowed| license.contains(allowed));
            if !has_permissive_choice {
                rejected.push(format!("{} {}: {license}", package.name, package.version));
            }
        }
        let result = if rejected.is_empty() {
            format!(
                "{} Cargo packages have an approved license choice\n",
                metadata.packages.len()
            )
        } else {
            format!("rejected licenses:\n{}\n", rejected.join("\n"))
        };
        fs::write(self.evidence.join("licenses.log"), &result)
            .map_err(|error| format!("cannot record license evidence: {error}"))?;
        if rejected.is_empty() {
            println!("ok");
            Ok(())
        } else {
            println!("FAILED");
            Err(format!(
                "license policy rejected {} packages",
                rejected.len()
            ))
        }
    }

    fn load_manifest(&self, path: &Path) -> Result<CorpusManifest, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read corpus manifest {}: {error}", path.display()))?;
        let manifest: CorpusManifest = toml::from_str(&text)
            .map_err(|error| format!("cannot parse corpus manifest {}: {error}", path.display()))?;
        if manifest.schema_version != 1 {
            return Err(format!(
                "unsupported corpus schema {}",
                manifest.schema_version
            ));
        }
        self.validate_manifest(&manifest)?;
        Ok(manifest)
    }

    fn validate_manifest(&self, manifest: &CorpusManifest) -> Result<(), String> {
        if manifest.schema_version != 1 || manifest.cases.is_empty() {
            return Err(
                "corpus manifest must use schema 1 and contain at least one case".to_owned(),
            );
        }
        for case in &manifest.cases {
            let valid_hash = case.sha256.len() == 64
                && case
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
            let valid_classification = matches!(
                case.classification.as_str(),
                "healthy_ansi" | "healthy_unicode" | "damaged" | "private_large"
            );
            if case.name.is_empty()
                || !case.path.is_absolute()
                || !valid_hash
                || !valid_classification
                || case.minimum_folders == 0
                || case.maximum_peak_stream_chunk_bytes > 65_536
                || (case.milestone_0_4_allow_oversize && !case.milestone_0_4)
            {
                return Err(format!(
                    "corpus case {} violates the manifest schema",
                    case.name
                ));
            }
        }
        for classification in ["healthy_ansi", "healthy_unicode"] {
            if !manifest
                .cases
                .iter()
                .any(|case| case.milestone_0_1_1 && case.classification == classification)
            {
                return Err(format!(
                    "the full gate requires a {classification} milestone_0_1_1 case"
                ));
            }
        }
        if !manifest.cases.iter().any(|case| {
            case.milestone_0_4
                && case.minimum_messages >= 2
                && case.milestone_0_4_max_pst_bytes >= 1024 * 1024
        }) {
            return Err(
                "the full gate requires a multi-message milestone_0_4 case with a limit of at least 1 MiB".to_owned(),
            );
        }
        Ok(())
    }

    fn prepare_full_corpus(&self, mut manifest: CorpusManifest) -> Result<PreparedCorpus, String> {
        type Generator = fn(&Path, &Path) -> Result<(), String>;
        let generators: [(&str, Generator); 6] = [
            (
                "v042-calendar-exception-source",
                qualify_calendar_exceptions,
            ),
            ("v042-distribution-list-source", qualify_distribution_list),
            ("v042-document-source", qualify_document_object),
            ("v042-ole-attachments-source", qualify_ole_attachments),
            (
                "v042-outlook-ole-object-source",
                qualify_outlook_ole_objects,
            ),
            (
                "v042-reference-attachments-source",
                qualify_reference_attachments,
            ),
        ];
        let scratch = tempfile::tempdir()
            .map_err(|error| format!("cannot create generated corpus scratch: {error}"))?;
        for case in &mut manifest.cases {
            if case.path.is_file() {
                continue;
            }
            let Some((_, generator)) = generators.iter().find(|(name, _)| *name == case.name)
            else {
                return Err(format!(
                    "corpus case {} is missing and has no deterministic generator: {}",
                    case.name,
                    case.path.display()
                ));
            };
            let output = scratch.path().join(&case.name);
            generator(&self.root, &output)?;
            let generated = output.join("parts/part-0001.pst");
            let identity = pstforge_core::SourceFile::open(&generated)
                .map_err(|error| format!("cannot inspect generated {}: {error}", case.name))?
                .identity()
                .clone();
            case.path = generated;
            case.sha256 = identity
                .sha256
                .ok_or_else(|| format!("generated {} has no SHA-256", case.name))?;
        }
        self.validate_manifest(&manifest)?;
        let manifest_path = scratch.path().join("full-manifest.toml");
        fs::write(
            &manifest_path,
            toml::to_string(&manifest)
                .map_err(|error| format!("cannot encode prepared corpus manifest: {error}"))?,
        )
        .map_err(|error| format!("cannot write prepared corpus manifest: {error}"))?;
        Ok(PreparedCorpus {
            manifest,
            manifest_path,
            _scratch: scratch,
        })
    }

    fn run_independent_readers(&self, manifest: &CorpusManifest) -> Result<(), String> {
        for case in manifest
            .cases
            .iter()
            .filter(|case| case.milestone_0_1 || case.milestone_0_1_1)
        {
            let path = case
                .path
                .to_str()
                .ok_or_else(|| format!("{} path is not UTF-8", case.name))?;
            self.redacted_reader(
                &format!("pffinfo-{}", sanitize(&case.name)),
                "pffinfo",
                &[path],
            )?;
            let output = tempfile::tempdir()
                .map_err(|error| format!("cannot create readpst scratch directory: {error}"))?;
            let output_path = output
                .path()
                .to_str()
                .ok_or_else(|| "readpst scratch path is not UTF-8".to_owned())?;
            self.redacted_reader(
                &format!("readpst-{}", sanitize(&case.name)),
                "readpst",
                &["-q", "-r", "-o", output_path, path],
            )?;
        }
        Ok(())
    }

    fn validate_generated_store(&self) -> Result<(), String> {
        let scratch = tempfile::tempdir()
            .map_err(|error| format!("cannot create writer scratch directory: {error}"))?;
        let pst = scratch.path().join("pstforge-writer-v0.2.1.pst");
        let fixture = pstforge_pst::writer::FidelityStore::default();
        let report = pstforge_pst::writer::create_fidelity_store(&pst, &fixture)
            .map_err(|error| format!("cannot create writer acceptance PST: {error}"))?;
        if !report.unsupported_properties.is_empty() {
            return Err("writer acceptance fixture omitted unsupported properties".to_owned());
        }
        let inventory = pstforge_core::verify(&pst)
            .map_err(|error| format!("libpff rejected writer acceptance PST: {error}"))?;
        let acceptable_attachment_count_issues = inventory.inventory.issues.is_empty()
            || inventory.inventory.issues.len() == 1
                && inventory.inventory.issues.iter().all(|issue| {
                    issue.operation == "count attachments"
                        && issue
                            .message
                            .contains("libpff_message_get_number_of_attachments")
                });
        if inventory.inventory.normal_items != 2
            || inventory.inventory.recipients != 4
            || inventory.inventory.attachments != 2
            || inventory.inventory.embedded_messages != 1
            || !acceptable_attachment_count_issues
        {
            return Err(format!(
                "libpff fidelity mismatch: items={}, recipients={}, attachments={}, embedded={}, issues={}",
                inventory.inventory.normal_items,
                inventory.inventory.recipients,
                inventory.inventory.attachments,
                inventory.inventory.embedded_messages,
                inventory.inventory.issues.len()
            ));
        }
        let source = fs::File::open(&pst)
            .map_err(|error| format!("cannot reopen writer PST for libpff fidelity: {error}"))?;
        let native = libpff_sys::PffFile::open_fd(source.as_fd())
            .map_err(|error| format!("libpff cannot open writer PST for fidelity: {error}"))?;
        let mut delivery = IndependentFidelitySink::default();
        native
            .catalog(&mut delivery)
            .map_err(|error| format!("libpff cannot catalog writer delivery times: {error}"))?;
        let expected_top = u64::try_from(fixture.message.received_filetime)
            .map_err(|_| "writer top-level received FILETIME is negative".to_owned())?;
        let expected_embedded = fixture
            .message
            .attachments
            .iter()
            .find_map(|attachment| match &attachment.content {
                pstforge_pst::writer::AttachmentContent::Embedded(message) => {
                    Some(message.received_filetime)
                }
                _ => None,
            })
            .ok_or_else(|| "writer fixture has no embedded message".to_owned())?;
        let expected_embedded = u64::try_from(expected_embedded)
            .map_err(|_| "writer embedded received FILETIME is negative".to_owned())?;
        if delivery.top_level != Some(expected_top) || delivery.embedded != Some(expected_embedded)
        {
            return Err(format!(
                "libpff delivery-time mismatch: top={:?}, embedded={:?}",
                delivery.top_level, delivery.embedded
            ));
        }
        validate_independent_properties(&delivery)?;
        let pst_path = pst
            .to_str()
            .ok_or_else(|| "writer acceptance path is not UTF-8".to_owned())?;
        self.redacted_reader("writer-pffinfo", "pffinfo", &[pst_path])?;

        let extracted = scratch.path().join("readpst");
        fs::create_dir(&extracted)
            .map_err(|error| format!("cannot create readpst writer output: {error}"))?;
        let extracted_path = extracted
            .to_str()
            .ok_or_else(|| "writer readpst path is not UTF-8".to_owned())?;
        self.redacted_reader(
            "writer-readpst",
            "readpst",
            &["-q", "-r", "-o", extracted_path, pst_path],
        )?;
        let required = [
            b"From: \"PSTForge Sender\" <sender@example.com>".as_slice(),
            b"To: Primary Recipient".as_slice(),
            b"Cc: Copy Recipient".as_slice(),
            b"Message-ID: <pstforge-fidelity@example.com>".as_slice(),
            b"Date: Wed, 01 Jan 2025 00:00:30 +0000".as_slice(),
            b"Plain-text body checkpoint.".as_slice(),
            "HTML body checkpoint: € 世界.".as_bytes(),
            b"e1xydGYxXGFuc2lcYiBSVEYgYm9keSBjaGVja3BvaW50LlxiMH0=".as_slice(),
            b"Embedded message checkpoint".as_slice(),
            b"From: \"Embedded Sender\" <embedded-sender@example.com>".as_slice(),
            b"To: Embedded Recipient".as_slice(),
            b"Date: Wed, 01 Jan 2025 00:00:10 +0000".as_slice(),
            b"Embedded plain-text body.".as_slice(),
            b"Content-ID: <checkpoint@pstforge>".as_slice(),
        ];
        if !directory_contains_file_with(&extracted, &required)? {
            return Err("readpst output did not contain the expected fidelity markers".to_owned());
        }
        let expected_attachment = match &fixture.message.attachments[0].content {
            pstforge_pst::writer::AttachmentContent::Binary(value) => value,
            _ => return Err("writer fixture first attachment is not binary".to_owned()),
        };
        let extracted_attachment = extract_base64_attachment(&extracted, "checkpoint.txt")?;
        if &extracted_attachment != expected_attachment {
            return Err("readpst binary attachment content mismatch".to_owned());
        }
        let attachment_hash = format!("{:x}", Sha256::digest(&extracted_attachment));
        fs::write(
            self.evidence.join("writer-acceptance.log"),
            format!(
                "generated bytes: {}\ninternal typed comparison: top-level and embedded metadata, To/Cc/Bcc roles and addresses, bodies, headers, timestamps, RTF sync/container, named/raw properties, record keys, attachment metadata, and complete payloads match\nlibpff: 2 items, 4 recipients, 2 attachments, 1 embedded, exact top-level/embedded delivery FILETIMEs, independently sampled top-level/embedded NAMEID values, raw Unicode/GUID values, and Bcc role/address\nlibpff issues: {} (only one explicit attachment-count uncertainty is permitted)\npffinfo: accepted\nreadpst: sender, To/Cc, headers, timestamps, text, HTML, exact RTF, embedded sender/recipient/time/body, inline metadata, and complete attachment extracted\nattachment bytes: {}\nattachment sha256: {}\n",
                pst.metadata()
                    .map_err(|error| format!("cannot inspect generated PST: {error}"))?
                    .len(),
                inventory.inventory.issues.len(),
                extracted_attachment.len(),
                attachment_hash,
            ),
        )
        .map_err(|error| format!("cannot record writer acceptance evidence: {error}"))?;
        println!("writer acceptance ... ok");
        Ok(())
    }

    fn redacted_reader(&self, name: &str, program: &str, args: &[&str]) -> Result<(), String> {
        print!("{name} ... ");
        let status = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|error| {
                format!("cannot run {program}; install pff-tools and pst-utils: {error}")
            })?;
        fs::write(
            self.evidence.join(format!("{name}.log")),
            format!(
                "reader: {program}\nstatus: {status}\noutput: redacted to protect PST content\n"
            ),
        )
        .map_err(|error| format!("cannot record {name} evidence: {error}"))?;
        if status.success() {
            println!("ok");
            Ok(())
        } else {
            println!("FAILED");
            Err(format!(
                "independent reader {program} failed for case {name}"
            ))
        }
    }
}

fn directory_contains_file_with(path: &Path, required: &[&[u8]]) -> Result<bool, String> {
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| {
                format!("cannot read an entry in {}: {error}", directory.display())
            })?;
            let metadata = entry
                .metadata()
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() && metadata.len() <= 1024 * 1024 {
                let data = fs::read(entry.path())
                    .map_err(|error| format!("cannot read {}: {error}", entry.path().display()))?;
                if required
                    .iter()
                    .all(|needle| data.windows(needle.len()).any(|window| window == *needle))
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn extract_base64_attachment(path: &Path, filename: &str) -> Result<Vec<u8>, String> {
    let mut pending = vec![path.to_path_buf()];
    let marker = format!("filename=\"{filename}\"");
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| {
                format!("cannot read an entry in {}: {error}", directory.display())
            })?;
            let metadata = entry
                .metadata()
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
                continue;
            }
            let content = fs::read_to_string(entry.path())
                .map_err(|error| format!("cannot read {}: {error}", entry.path().display()))?;
            let Some((_, after_marker)) = content.split_once(&marker) else {
                continue;
            };
            let normalized = after_marker.replace("\r\n", "\n");
            let Some((_, encoded_and_rest)) = normalized.split_once("\n\n") else {
                return Err(format!("attachment {filename} has no MIME body"));
            };
            let encoded = encoded_and_rest
                .lines()
                .take_while(|line| !line.is_empty() && !line.starts_with("--"))
                .collect::<String>();
            return base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| format!("cannot decode attachment {filename}: {error}"));
        }
    }
    Err(format!("readpst did not extract attachment {filename}"))
}

fn default_peak_chunk_limit() -> u64 {
    65_536
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read as _};
    use std::os::unix::fs::symlink;

    use super::{
        document_checkpoint_payload, prepare_generated_directory, rust_dependency_licenses,
    };

    #[test]
    fn packaged_rust_licenses_cover_only_the_executable_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or("cannot locate workspace root")?
            .to_path_buf();
        let licenses = rust_dependency_licenses(&root)?;
        assert!(licenses.contains("serde "));
        assert!(licenses.contains("rusqlite "));
        assert!(!licenses.contains("jsonschema "));
        assert!(licenses.contains("--- LICENSE"));
        Ok(())
    }

    #[test]
    fn package_cleanup_refuses_symlinked_target() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let sentinel = outside.path().join("keep");
        std::fs::write(&sentinel, b"operator data\n")?;
        symlink(outside.path(), workspace.path().join("target"))?;

        assert!(
            prepare_generated_directory(workspace.path(), &workspace.path().join("target/debian"))
                .is_err()
        );
        assert_eq!(std::fs::read(&sentinel)?, b"operator data\n");
        Ok(())
    }

    #[test]
    fn document_checkpoint_is_a_complete_minimal_opc_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = document_checkpoint_payload()?;
        let mut archive = zip::ZipArchive::new(Cursor::new(payload))?;
        let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            names,
            ["[Content_Types].xml", "_rels/.rels", "word/document.xml"]
        );

        let mut relationships = String::new();
        archive
            .by_name("_rels/.rels")?
            .read_to_string(&mut relationships)?;
        assert!(relationships.contains(
            r#"Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument""#
        ));
        assert!(relationships.contains(r#"Target="word/document.xml""#));

        let mut document = String::new();
        archive
            .by_name("word/document.xml")?
            .read_to_string(&mut document)?;
        assert!(document.contains("PSTForge document checkpoint"));
        Ok(())
    }
}
