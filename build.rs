use std::{
    env,
    path::PathBuf,
    process::{Command, ExitStatus},
};

fn main() {
    println!("cargo::rerun-if-changed=scripts/bootstrap-simplex.sh");
    println!("cargo::rerun-if-env-changed=SIMPLEX_SKIP_BOOTSTRAP");

    if env::var_os("SIMPLEX_SKIP_BOOTSTRAP").is_some() {
        println!("cargo::warning=Skipping libsimplex bootstrap (SIMPLEX_SKIP_BOOTSTRAP is set)");
        return;
    }

    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let bootstrap = root.join("scripts/bootstrap-simplex.sh");
    let status = run_bootstrap(&bootstrap, &root).unwrap_or_else(|error| {
        panic!("failed to start {}: {error}", bootstrap.display());
    });

    if !status.success() {
        panic!(
            "{} failed with status {status}; libsimplex is required before compiling simplex-tui",
            bootstrap.display()
        );
    }
}

fn run_bootstrap(script: &PathBuf, root: &PathBuf) -> std::io::Result<ExitStatus> {
    Command::new(script).current_dir(root).status()
}
