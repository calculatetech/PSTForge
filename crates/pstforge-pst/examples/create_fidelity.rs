use pstforge_pst::writer::{FidelityStore, create_fidelity_store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: create_fidelity <output.pst>")?;
    create_fidelity_store(path, &FidelityStore::default())?;
    Ok(())
}
