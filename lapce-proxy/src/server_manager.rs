use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::{Result, anyhow};
use flate2::read::GzDecoder;
use lapce_core::directory::Directory;
use serde_json::Value;
use tar::Archive;

// ── Node.js v20 LTS ──────────────────────────────────────────────────────────

struct NodeRelease {
    url: &'static str,
    /// Directory name inside the archive (e.g. "node-v20.19.1-darwin-arm64")
    dir: &'static str,
}

fn node_release() -> Option<NodeRelease> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(NodeRelease {
            url: "https://nodejs.org/dist/v20.19.1/node-v20.19.1-darwin-arm64.tar.gz",
            dir: "node-v20.19.1-darwin-arm64",
        }),
        ("macos", "x86_64") => Some(NodeRelease {
            url: "https://nodejs.org/dist/v20.19.1/node-v20.19.1-darwin-x64.tar.gz",
            dir: "node-v20.19.1-darwin-x64",
        }),
        ("linux", "x86_64") => Some(NodeRelease {
            url: "https://nodejs.org/dist/v20.19.1/node-v20.19.1-linux-x64.tar.gz",
            dir: "node-v20.19.1-linux-x64",
        }),
        ("linux", "aarch64") => Some(NodeRelease {
            url: "https://nodejs.org/dist/v20.19.1/node-v20.19.1-linux-arm64.tar.gz",
            dir: "node-v20.19.1-linux-arm64",
        }),
        _ => None,
    }
}

// ── clangd ───────────────────────────────────────────────────────────────────

fn clangd_zip_url() -> Option<&'static str> {
    match std::env::consts::OS {
        "macos" => Some(
            "https://github.com/clangd/clangd/releases/download/22.1.0/clangd-mac-22.1.0.zip",
        ),
        "linux" => Some(
            "https://github.com/clangd/clangd/releases/download/22.1.0/clangd-linux-22.1.0.zip",
        ),
        "windows" => Some(
            "https://github.com/clangd/clangd/releases/download/22.1.0/clangd-windows-22.1.0.zip",
        ),
        _ => None,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// HTTP client with a long timeout for large binary downloads (Node.js, clangd, etc.).
fn get_url(url: &str) -> Result<reqwest::blocking::Response> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let resp = client.get(url).send()?;
    Ok(resp)
}

fn download_tgz(url: &str, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut resp = get_url(url)?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {} for {}", resp.status(), url));
    }
    let decoder = GzDecoder::new(&mut resp);
    let mut archive = Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

/// Download an npm tarball and extract `package/` into `dest_modules/{name}/`.
fn download_npm(url: &str, name: &str, dest_modules: &Path) -> Result<()> {
    let final_dir = dest_modules.join(name);
    if final_dir.exists() {
        return Ok(());
    }
    let tmp = dest_modules.join(format!(".tmp-{name}"));
    if tmp.exists() {
        let _ = fs::remove_dir_all(&tmp);
    }
    fs::create_dir_all(&tmp)?;
    let mut resp = get_url(url)?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {} for {}", resp.status(), url));
    }
    let decoder = GzDecoder::new(&mut resp);
    let mut archive = Archive::new(decoder);
    archive.unpack(&tmp)?;
    // npm tarballs always extract into a `package/` subdirectory
    let src = tmp.join("package");
    if src.exists() {
        fs::rename(&src, &final_dir)?;
    } else {
        return Err(anyhow!("Expected package/ dir after extracting npm tarball"));
    }
    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

/// Read the `bin.{key}` field from a package.json and return the resolved path.
fn npm_bin_path(package_dir: &Path, bin_key: &str) -> Option<PathBuf> {
    let content = fs::read_to_string(package_dir.join("package.json")).ok()?;
    let pkg: Value = serde_json::from_str(&content).ok()?;
    let rel = pkg.get("bin")?.get(bin_key)?.as_str()?;
    Some(package_dir.join(rel.trim_start_matches("./")))
}

/// Write a shell wrapper that calls `node {main_js}` with forwarded args, then make it executable.
fn write_node_wrapper(wrapper: &Path, node: &Path, main_js: &Path) -> Result<()> {
    if let Some(parent) = wrapper.parent() {
        fs::create_dir_all(parent)?;
    }
    let script = format!(
        "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
        node.display(),
        main_js.display()
    );
    fs::write(wrapper, &script)?;
    make_executable(wrapper)?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Recursively search for a file with `name` under `root`, returning the first match.
fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(root).ok()?;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

// ── Per-server ensure functions ───────────────────────────────────────────────

fn ensure_node(servers: &Path) -> Option<PathBuf> {
    let rel = node_release()?;
    let bin = servers
        .join("node")
        .join(rel.dir)
        .join("bin")
        .join("node");
    if bin.exists() {
        return Some(bin);
    }
    tracing::info!("Downloading Node.js…");
    if let Err(e) = download_tgz(rel.url, &servers.join("node")) {
        tracing::warn!("Node.js download failed: {:?}", e);
        return None;
    }
    make_executable(&bin).ok()?;
    Some(bin)
}

fn ensure_typescript_ls(servers: &Path, node: &Path) -> Option<PathBuf> {
    let wrapper = servers.join("typescript-ls").join("typescript-language-server");
    if wrapper.exists() {
        return Some(wrapper);
    }
    let modules = servers.join("typescript-ls").join("node_modules");
    tracing::info!("Downloading typescript-language-server…");
    if let Err(e) = download_npm(
        "https://registry.npmjs.org/typescript-language-server/-/typescript-language-server-5.1.3.tgz",
        "typescript-language-server",
        &modules,
    ) {
        tracing::warn!("typescript-language-server download failed: {:?}", e);
        return None;
    }
    tracing::info!("Downloading typescript…");
    if let Err(e) = download_npm(
        "https://registry.npmjs.org/typescript/-/typescript-6.0.3.tgz",
        "typescript",
        &modules,
    ) {
        tracing::warn!("typescript download failed: {:?}", e);
        return None;
    }
    let pkg_dir = modules.join("typescript-language-server");
    let main_js = npm_bin_path(&pkg_dir, "typescript-language-server")
        .unwrap_or_else(|| pkg_dir.join("lib").join("cli.mjs"));
    if !main_js.exists() {
        tracing::warn!("typescript-language-server entry not found at {:?}", main_js);
        return None;
    }
    if let Err(e) = write_node_wrapper(&wrapper, node, &main_js) {
        tracing::warn!("Failed to write typescript-language-server wrapper: {:?}", e);
        return None;
    }
    Some(wrapper)
}

fn ensure_pyright(servers: &Path, node: &Path) -> Option<PathBuf> {
    let wrapper = servers.join("pyright").join("pyright-langserver");
    if wrapper.exists() {
        return Some(wrapper);
    }
    let modules = servers.join("pyright").join("node_modules");
    tracing::info!("Downloading pyright…");
    if let Err(e) = download_npm(
        "https://registry.npmjs.org/pyright/-/pyright-1.1.409.tgz",
        "pyright",
        &modules,
    ) {
        tracing::warn!("pyright download failed: {:?}", e);
        return None;
    }
    let pkg_dir = modules.join("pyright");
    let main_js = npm_bin_path(&pkg_dir, "pyright-langserver")
        .unwrap_or_else(|| pkg_dir.join("dist").join("pyright-langserver.bundle.js"));
    if !main_js.exists() {
        tracing::warn!("pyright entry not found at {:?}", main_js);
        return None;
    }
    if let Err(e) = write_node_wrapper(&wrapper, node, &main_js) {
        tracing::warn!("Failed to write pyright wrapper: {:?}", e);
        return None;
    }
    Some(wrapper)
}

fn ensure_clangd(servers: &Path) -> Option<PathBuf> {
    let dest = servers.join("clangd");
    let bin_name = if cfg!(windows) { "clangd.exe" } else { "clangd" };
    // Check if we already extracted it
    if let Some(existing) = find_file(&dest, bin_name) {
        return Some(existing);
    }
    let url = clangd_zip_url()?;
    tracing::info!("Downloading clangd…");
    let mut resp = match get_url(url) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("clangd download failed: {:?}", e);
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!("clangd download HTTP {}", resp.status());
        return None;
    }
    fs::create_dir_all(&dest).ok()?;
    // Write zip to temp file then extract
    let zip_path = dest.join("clangd.zip");
    {
        let mut f = fs::File::create(&zip_path).ok()?;
        io::copy(&mut resp, &mut f).ok()?;
    }
    if let Err(e) = extract_zip(&zip_path, &dest) {
        tracing::warn!("clangd extraction failed: {:?}", e);
        let _ = fs::remove_file(&zip_path);
        return None;
    }
    let _ = fs::remove_file(&zip_path);
    let bin = find_file(&dest, bin_name)?;
    make_executable(&bin).ok()?;
    Some(bin)
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let f = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(f)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let out = dest.join(entry.name());
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut outf = fs::File::create(&out)?;
            io::copy(&mut entry, &mut outf)?;
            #[cfg(unix)]
            {
                // Preserve executable bit from zip metadata
                if let Some(mode) = entry.unix_mode() {
                    let mut perms = outf.metadata()?.permissions();
                    perms.set_mode(mode);
                    fs::set_permissions(&out, perms)?;
                }
            }
        }
    }
    Ok(())
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Download any missing language servers and return plugin config entries with their paths.
/// Only keys not already present in `existing_configs` are injected (user settings win).
pub fn ensure_servers(
    existing_configs: &HashMap<String, HashMap<String, Value>>,
) -> HashMap<String, HashMap<String, Value>> {
    let mut result: HashMap<String, HashMap<String, Value>> = HashMap::new();

    let Some(dir) = Directory::data_local_directory().map(|d| d.join("servers")) else {
        return result;
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        tracing::warn!("Failed to create servers dir: {:?}", e);
        return result;
    }

    let maybe_set = |result: &mut HashMap<String, HashMap<String, Value>>,
                     plugin: &str,
                     key: &str,
                     path: PathBuf| {
        // Skip if the user already configured this key
        let already_set = existing_configs
            .get(plugin)
            .and_then(|m| m.get(key))
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        if !already_set {
            result
                .entry(plugin.to_string())
                .or_default()
                .insert(key.to_string(), Value::String(path.to_string_lossy().into_owned()));
        }
    };

    let node = ensure_node(&dir);

    if let Some(ref node_path) = node {
        if let Some(ts) = ensure_typescript_ls(&dir, node_path) {
            maybe_set(&mut result, "lapce-typescript", "volt.serverPath", ts);
        }
        if let Some(py) = ensure_pyright(&dir, node_path) {
            maybe_set(&mut result, "lapce-python", "serverPath", py);
        }
    }

    if let Some(clangd) = ensure_clangd(&dir) {
        maybe_set(&mut result, "lapce-cpp-clangd", "volt.serverPath", clangd);
    }

    result
}
