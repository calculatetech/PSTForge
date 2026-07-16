use std::fs;
use std::process::Command;

#[test]
fn invalid_source_fails_before_creating_job() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let source = temporary.path().join("not-a-pst.bin");
    let job = temporary.path().join("job");
    fs::write(&source, b"not a pst").expect("invalid source fixture");

    let status = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .args(["recover", source.to_str().expect("source path"), "--output"])
        .arg(&job)
        .status()
        .expect("run pstforge");

    assert_eq!(status.code(), Some(3));
    assert!(!job.exists());
}
