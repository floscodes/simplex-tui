use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

fn main() {
    for script in [
        "scripts/bootstrap-simplex.sh",
        "scripts/bootstrap-simplex-macos.sh",
        "scripts/bootstrap-simplex-windows.ps1",
    ] {
        println!("cargo::rerun-if-changed={script}");
    }
    println!("cargo::rerun-if-env-changed=SIMPLEX_SKIP_BOOTSTRAP");

    if env::var_os("SIMPLEX_SKIP_BOOTSTRAP").is_some() {
        println!("cargo::warning=Skipping libsimplex bootstrap (SIMPLEX_SKIP_BOOTSTRAP is set)");
        return;
    }

    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let host = env::var("HOST").expect("missing HOST");
    let target = env::var("TARGET").expect("missing TARGET");
    if host != target {
        panic!("libsimplex only supports native builds (host {host}, target {target})");
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("missing CARGO_CFG_TARGET_OS");
    let (program, args): (&str, Vec<&str>) = match target_os.as_str() {
        "linux" => ("bash", vec!["scripts/bootstrap-simplex.sh"]),
        "macos" => ("bash", vec!["scripts/bootstrap-simplex-macos.sh"]),
        "windows" => (
            "powershell.exe",
            vec![
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "scripts/bootstrap-simplex-windows.ps1",
            ],
        ),
        other => panic!("libsimplex bootstrap is not supported on {other}"),
    };
    let status = run_bootstrap(program, &args, &root).unwrap_or_else(|error| {
        panic!("failed to start libsimplex bootstrap for {target_os}: {error}");
    });

    if !status.success() {
        panic!(
            "libsimplex bootstrap for {target_os} failed with status {status}; \
             libsimplex is required before compiling simplex-tui"
        );
    }
}

fn run_bootstrap(program: &str, args: &[&str], root: &Path) -> std::io::Result<ExitStatus> {
    Command::new(program).args(args).current_dir(root).status()
}
