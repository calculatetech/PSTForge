#![deny(unsafe_code)]

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    path: PathBuf,
    sha256: String,
    classification: String,
    milestone_0_1: bool,
    minimum_folders: u64,
    minimum_messages: u64,
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_1_external_psts_are_inspected_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest_text = fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = toml::from_str(&manifest_text)?;
    if manifest.schema_version != 1 {
        return Err(format!("unsupported corpus schema {}", manifest.schema_version).into());
    }
    let cases: Vec<&Case> = manifest
        .cases
        .iter()
        .filter(|case| case.milestone_0_1)
        .collect();
    if cases.is_empty() {
        return Err("manifest has no milestone_0_1 cases".into());
    }

    for case in cases {
        if !matches!(
            case.classification.as_str(),
            "healthy_ansi" | "healthy_unicode"
        ) {
            return Err(format!("{} is not classified as a healthy PST", case.name).into());
        }
        let before_metadata = fs::metadata(&case.path)?;
        let before_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != case.sha256 {
            return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
        }

        let info = run_json("info", case)?;
        if info["source"]["sha256"] != case.sha256 {
            return Err(format!("{} info returned a different SHA-256", case.name).into());
        }
        let verify = run_json("verify", case)?;
        let folders = verify["inventory"]["folders"].as_u64().unwrap_or_default();
        let messages = verify["inventory"]["normal_items"]
            .as_u64()
            .unwrap_or_default();
        if folders < case.minimum_folders || messages < case.minimum_messages {
            return Err(format!("{} inventory is below manifest minimums", case.name).into());
        }

        let after_metadata = fs::metadata(&case.path)?;
        let after_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != after_hash
            || before_metadata.len() != after_metadata.len()
            || modified_ns(&before_metadata)? != modified_ns(&after_metadata)?
            || accessed_ns(&before_metadata) != accessed_ns(&after_metadata)
        {
            return Err(format!("{} changed during inspection", case.name).into());
        }
    }
    Ok(())
}

fn run_json(command: &str, case: &Case) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg(command)
        .arg(&case.path)
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "{} failed for {}: {}",
            command,
            case.name,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn modified_ns(metadata: &fs::Metadata) -> Result<std::time::SystemTime, std::io::Error> {
    metadata.modified()
}

fn accessed_ns(metadata: &fs::Metadata) -> (i64, i64) {
    (metadata.atime(), metadata.atime_nsec())
}
