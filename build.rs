// Puts the commit in `--version`, since installs come straight from the main
// branch and the version number alone does not say which build you have.
fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let full = if commit.is_empty() {
        version.to_string()
    } else {
        format!("{version} ({commit})")
    };
    println!("cargo:rustc-env=RECAP_VERSION={full}");
    // Run again after a commit or checkout so the commit shown stays current.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
