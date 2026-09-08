use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ZQS_BUILD_COMMIT");
    if let Ok(commit) = std::env::var("ZQS_BUILD_COMMIT") {
        if !commit.is_empty() && commit.chars().all(|c| c.is_ascii_hexdigit()) {
            println!("cargo:rustc-env=GIT_COMMIT_HASH={commit}");
            return;
        }
    }
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        if output.status.success() {
            if let Ok(hash) = String::from_utf8(output.stdout) {
                let trimmed = hash.trim();
                if !trimmed.is_empty() {
                    println!("cargo:rustc-env=GIT_COMMIT_HASH={trimmed}");
                }
            }
        }
    }
}
