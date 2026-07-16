use std::{env, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing output path"))?;
    pstforge_pst::writer::create_minimal_store(
        path,
        &pstforge_pst::writer::MinimalStore::default(),
    )?;
    Ok(())
}
