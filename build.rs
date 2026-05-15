use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dev".to_string());

    let build_time = build_time_utc();

    println!("cargo:rustc-env=GIT_COMMIT={commit}");
    println!("cargo:rustc-env=BUILD_TIME={build_time}");
}

fn build_time_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    match Command::new("date")
        .args(["-u", "-d", &format!("@{secs}"), "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .unwrap_or_default()
            .trim()
            .to_string(),
        _ => format!("unix:{secs}"),
    }
}
