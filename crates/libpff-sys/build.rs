fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/bindings.rs");

    if let Err(error) = pkg_config::Config::new()
        .atleast_version("20180714")
        .probe("libpff")
    {
        eprintln!(
            "libpff >= 20180714 is required; on Debian/Ubuntu install \
             libpff-dev (pkg-config error: {error})"
        );
        std::process::exit(1);
    }
}
