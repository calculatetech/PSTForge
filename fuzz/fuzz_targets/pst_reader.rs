#![no_main]

use std::io::Write as _;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut input) = tempfile::NamedTempFile::new() else {
        return;
    };
    if input.write_all(data).is_err() || input.flush().is_err() {
        return;
    }
    let _ = pstforge_pst::open_store(input.path());
});
