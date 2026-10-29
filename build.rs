use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=frontend/out");
    println!("cargo:rerun-if-changed=install-client.sh");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_ROUTER_BUILD_COMMIT");

    let commit = std::env::var("CC_SWITCH_ROUTER_BUILD_COMMIT")
        .ok()
        .map(|value| {
            assert!(
                value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "CC_SWITCH_ROUTER_BUILD_COMMIT must be a full Git commit SHA"
            );
            value[..7].to_ascii_lowercase()
        })
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
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

    let gzip_dir = Path::new(&out_dir).join("ui_gzip");
    fs::create_dir_all(&gzip_dir).expect("create gzip asset dir");
    let mut gzip_files = std::collections::HashSet::new();

    let mut body = String::new();
    body.push_str(
        "pub struct UiAsset {\n\
         \tpub path: &'static str,\n\
         \tpub bytes: &'static [u8],\n\
         \tpub gzip: Option<&'static [u8]>,\n\
         \tpub etag: &'static str,\n\
         \tpub content_type: &'static str,\n\
         \tpub immutable: bool,\n\
         }\n\n\
         pub fn ui_asset(path: &str) -> Option<UiAsset> {\n\
         \tmatch path {\n",
    );
    for (rel, abs) in files {
        let content_type = content_type_for(&rel);
        let immutable = rel.starts_with("_next/static/");
        let bytes = fs::read(&abs).expect("read frontend asset");
        let digest = asset_digest(&bytes);
        let etag = format!("\"{digest}\"");
        let gzip = gzip_asset(&gzip_dir, &digest, content_type, &bytes, &mut gzip_files)
            .map(|path| format!("Some(include_bytes!({:?}))", path.display().to_string()))
            .unwrap_or_else(|| "None".to_string());
        body.push_str(&format!(
            "\t\t{rel:?} => Some(UiAsset {{ path: {rel:?}, bytes: include_bytes!({abs:?}), gzip: {gzip}, etag: {etag:?}, content_type: {content_type:?}, immutable: {immutable} }}),\n",
            rel = rel,
            abs = abs.display().to_string(),
            gzip = gzip,
            etag = etag,
            content_type = content_type,
            immutable = immutable,
        ));
    }
    body.push_str("\t\t_ => None,\n\t}\n}\n");
    fs::write(dest, body).expect("write generated ui asset table");
    prune_gzip_dir(&gzip_dir, &gzip_files);
}

/// Content hash used as the strong validator for conditional revalidation
/// (HTML and RSC payloads are `no-cache`, so every deploy is picked up while
/// unchanged files cost a 304) and as the name of the cached gzip file.
fn asset_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Precompress text assets once so serving them costs no CPU. Results are
/// content-addressed, so a rerun of this script (any git index change) only
/// compresses files that changed. Small files and files that barely shrink
/// are served as-is; a `.skip` marker remembers that verdict.
fn gzip_asset(
    gzip_dir: &Path,
    digest: &str,
    content_type: &str,
    bytes: &[u8],
    used: &mut std::collections::HashSet<String>,
) -> Option<PathBuf> {
    let compressible = content_type.starts_with("text/")
        || content_type.starts_with("application/javascript")
        || content_type.starts_with("application/json")
        || content_type.starts_with("image/svg+xml");
    if !compressible || bytes.len() < 1024 {
        return None;
    }
    let path = gzip_dir.join(format!("{digest}.gz"));
    let skip = gzip_dir.join(format!("{digest}.skip"));
    if path.exists() {
        used.insert(format!("{digest}.gz"));
        return Some(path);
    }
    if skip.exists() {
        used.insert(format!("{digest}.skip"));
        return None;
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(9));
    encoder.write_all(bytes).expect("gzip frontend asset");
    let compressed = encoder.finish().expect("finish gzip frontend asset");
    if compressed.len() * 10 > bytes.len() * 9 {
        fs::write(&skip, b"").expect("write gzip skip marker");
        used.insert(format!("{digest}.skip"));
        return None;
    }
    fs::write(&path, compressed).expect("write gzip frontend asset");
    used.insert(format!("{digest}.gz"));
    Some(path)
}

fn prune_gzip_dir(gzip_dir: &Path, used: &std::collections::HashSet<String>) {
    let Ok(entries) = fs::read_dir(gzip_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if used.contains(&name) {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
    }
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
