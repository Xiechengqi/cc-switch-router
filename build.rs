use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=frontend/out");
    println!("cargo:rerun-if-changed=install-client.sh");

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
    generate_ui_assets();
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

fn generate_ui_assets() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let dest = Path::new(&out_dir).join("ui_assets.rs");
    let ui_root = Path::new("frontend/out");
    let index_html = ui_root.join("index.html");
    if std::env::var("PROFILE").as_deref() == Ok("release") && !index_html.exists() {
        panic!(
            "frontend assets are missing; run `(cd frontend && npm ci && npm run build)` before `cargo build --release`"
        );
    }
    let mut files = Vec::new();
    if ui_root.exists() {
        collect_files(ui_root, ui_root, &mut files);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut body = String::new();
    body.push_str(
        "pub struct UiAsset {\n\
         \tpub path: &'static str,\n\
         \tpub bytes: &'static [u8],\n\
         \tpub content_type: &'static str,\n\
         \tpub immutable: bool,\n\
         }\n\n\
         pub fn ui_asset(path: &str) -> Option<UiAsset> {\n\
         \tmatch path {\n",
    );
    for (rel, abs) in files {
        let content_type = content_type_for(&rel);
        let immutable = rel.starts_with("_next/static/");
        body.push_str(&format!(
            "\t\t{rel:?} => Some(UiAsset {{ path: {rel:?}, bytes: include_bytes!({abs:?}), content_type: {content_type:?}, immutable: {immutable} }}),\n",
            rel = rel,
            abs = abs.display().to_string(),
            content_type = content_type,
            immutable = immutable,
        ));
    }
    body.push_str("\t\t_ => None,\n\t}\n}\n");
    fs::write(dest, body).expect("write generated ui asset table");
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .expect("path under root")
            .to_string_lossy()
            .replace('\\', "/");
        let abs = fs::canonicalize(&path).unwrap_or(path);
        files.push((rel, abs));
    }
}

fn content_type_for(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
