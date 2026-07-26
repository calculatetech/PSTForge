include!("../../build-support/libpff_rpath.rs");

fn main() {
    if let Err(error) = configure_libpff_runtime_path() {
        eprintln!("cannot configure the pinned libpff runtime path: {error}");
        std::process::exit(1);
    }
}
