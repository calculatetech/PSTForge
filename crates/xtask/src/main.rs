#![deny(unsafe_code)]

use std::fs;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusManifest {
    schema_version: u32,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize)]
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
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    license: Option<String>,
}

struct Gate {
    root: PathBuf,
    evidence: PathBuf,
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
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("gate")) {
        return Err("usage: cargo xtask gate <fast|full|release>".to_owned());
    }
    let tier = arguments
        .next()
        .ok_or_else(|| "missing gate tier: fast, full, or release".to_owned())?;
    if arguments.next().is_some() {
        return Err("unexpected arguments after gate tier".to_owned());
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot locate workspace root".to_owned())?
        .to_path_buf();
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
        self.command(
            "external-corpus",
            "cargo",
            &[
                "test",
                "-p",
                "pstforge-cli",
                "--test",
                "external_corpus",
                "--locked",
                "--",
                "--ignored",
                "--nocapture",
            ],
        )?;
        self.run_independent_readers(&manifest)?;
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
            "tests/corpus-schema.json",
            "tests/corpus-manifest.example.toml",
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
