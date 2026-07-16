#![deny(unsafe_code)]

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

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
    #[serde(default)]
    milestone_0_1_1: bool,
    minimum_folders: u64,
    minimum_messages: u64,
    #[serde(default)]
    minimum_recipients: u64,
    #[serde(default)]
    minimum_attachments: u64,
    #[serde(default)]
    minimum_raw_properties: u64,
    #[serde(default = "default_peak_chunk_limit")]
    maximum_peak_stream_chunk_bytes: u64,
    #[serde(default)]
    milestone_0_3: bool,
    #[serde(default)]
    minimum_recovered_items: u64,
    #[serde(default)]
    minimum_orphan_items: u64,
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_external_recovery_spools_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    if manifest.schema_version != 1 {
        return Err(format!("unsupported corpus schema {}", manifest.schema_version).into());
    }
    let cases: Vec<&Case> = manifest
        .cases
        .iter()
        .filter(|case| case.milestone_0_3 || case.classification == "damaged")
        .collect();
    if cases.is_empty() {
        return Err("manifest has no milestone_0_3 or damaged cases".into());
    }

    for case in cases {
        let before_metadata = fs::metadata(&case.path)?;
        let before_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != case.sha256 {
            return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
        }
        let directory = tempfile::tempdir()?;
        let job = directory.path().join("job");
        let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
            .arg("recover")
            .arg(&case.path)
            .arg("--output")
            .arg(&job)
            .arg("--json")
            .arg("--color")
            .arg("never")
            .output()?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(format!(
                "recover failed for {}: {}",
                case.name,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let normal = report["normal_items"].as_u64().unwrap_or_default();
        let recovered = report["recovered_items"].as_u64().unwrap_or_default();
        let orphan = report["orphan_items"].as_u64().unwrap_or_default();
        let committed = report["committed_candidates"].as_u64().unwrap_or_default();
        if normal < case.minimum_messages
            || recovered < case.minimum_recovered_items
            || orphan < case.minimum_orphan_items
            || committed != normal + recovered + orphan
        {
            return Err(format!(
                "{} recovery totals violate manifest expectations",
                case.name
            )
            .into());
        }
        if !job.join(".pstforge/job.sqlite3").is_file() {
            return Err(format!("{} did not produce a durable job ledger", case.name).into());
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
            return Err(format!("{} changed during recovery", case.name).into());
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_aggressive_recovery_is_distinct_and_non_mutating()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .min_by_key(|case| {
            fs::metadata(&case.path)
                .map(|metadata| metadata.len())
                .unwrap_or(u64::MAX)
        })
        .ok_or("manifest has no recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if before.sha256 != case.sha256 {
        return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
    }
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--recovery")
        .arg("aggressive")
        .arg("--json")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["mode"], "aggressive");
    let committed = report["committed_candidates"].as_u64().unwrap_or_default();
    let normal = report["normal_items"].as_u64().unwrap_or_default();
    let recovered = report["recovered_items"].as_u64().unwrap_or_default();
    let orphan = report["orphan_items"].as_u64().unwrap_or_default();
    let fragments = report["fragment_items"].as_u64().unwrap_or_default();
    assert_eq!(committed, normal + recovered + orphan + fragments);
    let sink = pstforge_job::DurableCatalogSink::open(&job)?;
    let summary = sink.summary()?;
    assert_eq!(summary.committed_candidates, committed);
    assert_eq!(summary.recovered_candidates, recovered);
    assert_eq!(summary.orphan_candidates, orphan);
    assert_eq!(summary.fragment_candidates, fragments);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_worker_abort_replays_committed_candidates()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_AFTER_CANDIDATES", "1")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_persistent_worker_abort_is_bounded_and_partial()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_EVERY_ATTEMPT_AFTER_CANDIDATES", "1")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 4);
    assert_eq!(report["worker_failures"], 4);
    assert_eq!(report["committed_candidates"], 1);
    assert_eq!(report["issues"], 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_worker_stall_is_killed_and_replayed() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_STALL_AFTER_CANDIDATES", "1")
        .env("PSTFORGE_TEST_STALL_TIMEOUT_MS", "1000")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_repeated_unit_crash_is_isolated() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 2
        })
        .ok_or("manifest has no recovery case with at least three messages")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_ON_UNIT_ORDINAL", "2")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 5);
    assert_eq!(report["worker_failures"], 4);
    assert_eq!(report["isolated_units"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert!(report["issues"].as_u64().unwrap_or_default() >= 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_replayed_candidate_does_not_prevent_unit_isolation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 2
        })
        .ok_or("manifest has no recovery case with at least three messages")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_INSIDE_UNIT_AFTER_CANDIDATES", "1")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 5);
    assert_eq!(report["worker_failures"], 4);
    assert_eq!(report["isolated_units"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_sigsegv_is_contained_and_isolated() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.milestone_0_3 || case.classification == "damaged")
        .ok_or("manifest has no damaged recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_SEGV_ON_UNIT_ORDINAL", "2")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 5);
    assert_eq!(report["worker_failures"], 4);
    assert_eq!(report["isolated_units"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert!(job.join(".pstforge/job.sqlite3").is_file());
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_parser_error_after_commit_replays_and_continues()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_PARSER_ERROR_AFTER_CANDIDATES", "1")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_sigint_and_sigterm_leave_durable_partial_jobs()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.milestone_0_3 || case.classification == "damaged")
        .ok_or("manifest has no damaged recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    for signal in [rustix::process::Signal::INT, rustix::process::Signal::TERM] {
        let directory = tempfile::tempdir()?;
        let job = directory.path().join("job");
        let mut child = Command::new(env!("CARGO_BIN_EXE_pstforge"))
            .arg("recover")
            .arg(&case.path)
            .arg("--output")
            .arg(&job)
            .arg("--json")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.join(".pstforge/job.sqlite3").is_file() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err("recovery job did not start before signal deadline".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        thread::sleep(Duration::from_millis(500));
        let pid = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or("child PID is out of range")?;
        rustix::process::kill_process(pid, signal)?;
        let output = child.wait_with_output()?;
        assert_eq!(output.status.code(), Some(130));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(report["interrupted"], true);
        assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 0);
        assert!(job.join(".pstforge/job.sqlite3").is_file());
    }
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
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
        .filter(|case| case.milestone_0_1 || case.milestone_0_1_1)
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
        if case.milestone_0_1_1 {
            let recipients = verify["inventory"]["recipients"]
                .as_u64()
                .unwrap_or_default();
            let attachments = verify["inventory"]["attachments"]
                .as_u64()
                .unwrap_or_default();
            let properties = verify["inventory"]["raw_properties"]
                .as_u64()
                .unwrap_or_default();
            let peak = verify["inventory"]["peak_stream_chunk_bytes"]
                .as_u64()
                .unwrap_or(u64::MAX);
            if recipients < case.minimum_recipients
                || attachments < case.minimum_attachments
                || properties < case.minimum_raw_properties
                || peak > case.maximum_peak_stream_chunk_bytes
            {
                return Err(format!("{} catalog is outside manifest invariants", case.name).into());
            }
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

fn default_peak_chunk_limit() -> u64 {
    65_536
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
