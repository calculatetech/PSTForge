#![deny(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

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
    #[serde(rename = "minimum_messages")]
    _minimum_messages: u64,
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
            {
                return Err(format!(
                    "corpus case {} violates the manifest schema",
                    case.name
                ));
            }
        }
        if env!("CARGO_PKG_VERSION") == "0.1.1" {
            for classification in ["healthy_ansi", "healthy_unicode"] {
                if !manifest
                    .cases
                    .iter()
                    .any(|case| case.milestone_0_1_1 && case.classification == classification)
                {
                    return Err(format!(
                        "version 0.1.1 requires a {classification} milestone_0_1_1 case"
                    ));
                }
            }
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
