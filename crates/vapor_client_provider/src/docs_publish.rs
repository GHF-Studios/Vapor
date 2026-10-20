//! Curated product documentation packaging and publication.

use crate::discovery::EnvironmentPaths;
use reqwest::{Url, blocking::Client, header::AUTHORIZATION};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const DEFAULT_DOCS_BASE_URL: &str = "https://vapor.ghf-studios.site/docs";
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Build and upload the curated public Vapor documentation site.
///
/// The endpoint can be overridden with `VAPOR_DOCS_BASE_URL`; its admin token
/// must be provided through `VAPOR_DOCS_ADMIN_TOKEN` in the Client service's
/// environment. Public HTTP endpoints are rejected so the token is never sent
/// over cleartext transport; HTTP is allowed only for a loopback test server.
pub fn publish(paths: &EnvironmentPaths) -> Result<String, String> {
    let token = env::var("VAPOR_DOCS_ADMIN_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            "VAPOR_DOCS_ADMIN_TOKEN is not configured for the Vapor Client service".to_owned()
        })?;
    let base_url = env::var("VAPOR_DOCS_BASE_URL").unwrap_or_else(|_| DEFAULT_DOCS_BASE_URL.into());
    let upload_url = upload_url(&base_url)?;
    let bundle = build_bundle(paths.source().root())?;
    if bundle.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "documentation archive is {} bytes; server upload limit is {MAX_UPLOAD_BYTES} bytes",
            bundle.len()
        ));
    }

    let bundle_bytes = bundle.len();
    let response_text = upload_bundle(&upload_url, &token, bundle)?;
    Ok(format!(
        "Published Vapor documentation ({} compressed bytes). {}",
        bundle_bytes,
        response_text.trim()
    ))
}

fn upload_bundle(url: &Url, token: &str, bundle: Vec<u8>) -> Result<String, String> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| format!("could not prepare the documentation upload: {error}"))?;
    let response = client
        .post(url.clone())
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::CONTENT_TYPE, "application/gzip")
        .body(bundle)
        .send()
        .map_err(|error| format!("could not upload Vapor documentation: {error}"))?;
    let status = response.status();
    let response_text = response.text().map_err(|error| {
        format!("uploaded documentation but could not read server response: {error}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "Vapor Docs rejected the upload ({status}): {}",
            response_text.trim()
        ));
    }
    Ok(response_text.trim().to_owned())
}

fn upload_url(base_url: &str) -> Result<Url, String> {
    let mut url = Url::parse(base_url.trim_end_matches('/'))
        .map_err(|error| format!("invalid VAPOR_DOCS_BASE_URL: {error}"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "VAPOR_DOCS_BASE_URL must not contain credentials, a query, or a fragment".to_owned(),
        );
    }
    let loopback = match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(
            "Vapor Docs publication requires HTTPS; HTTP is allowed only for a loopback test server"
                .to_owned(),
        );
    }
    let path = format!("{}/current.tar.gz", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}

fn build_bundle(source_root: &Path) -> Result<Vec<u8>, String> {
    let client_root = if source_root.join("App-Source.vapor.toml").is_file() {
        source_root.to_path_buf()
    } else {
        source_root.join("Vapor-Client")
    };
    if !client_root.join("App-Source.vapor.toml").is_file() {
        return Err(format!(
            "Vapor Client sources were not found under '{}'",
            source_root.display()
        ));
    }
    let staging = TemporarySite::create()?;
    let site = staging.path.join("site");
    fs::create_dir_all(&site).map_err(|error| error.to_string())?;

    for name in ["README.md", "App.vapor.toml", "App-Source.vapor.toml"] {
        let from = client_root.join(name);
        if from.is_file() {
            let target = site.join("client");
            fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            fs::copy(&from, target.join(name)).map_err(|error| {
                format!("could not stage public file '{}': {error}", from.display())
            })?;
        }
    }

    copy_tree_if_present(
        &client_root.join("Vapor-CLI/crates/vapor_cli/docs"),
        &site.join("shell"),
    )?;
    copy_tree_if_present(
        &client_root.join("Vapor/docs/roadmap"),
        &site.join("vapor/roadmap"),
    )?;
    let book_links = build_books(&client_root.join("Vapor/docs/books"), &site)?;
    write_site_index(&site, &book_links)?;
    validate_site_tree(&site)?;

    let output = Command::new("tar")
        .args(["-czf", "-", "-C"])
        .arg(&site)
        .arg(".")
        .output()
        .map_err(|error| format!("could not start the system tar utility: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not archive Vapor documentation: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

struct TemporarySite {
    path: PathBuf,
}

impl TemporarySite {
    fn create() -> Result<Self, String> {
        let root = env::temp_dir();
        for attempt in 0..16 {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = root.join(format!(
                "vapor-docs-{}-{stamp}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "could not create docs staging directory '{}': {error}",
                        path.display()
                    ));
                }
            }
        }
        Err("could not reserve a unique docs staging directory".to_owned())
    }
}

impl Drop for TemporarySite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn build_books(books_root: &Path, site: &Path) -> Result<Vec<(String, String)>, String> {
    if !books_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut books = fs::read_dir(books_root)
        .map_err(|error| format!("could not read '{}': {error}", books_root.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("book.toml"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    books.sort();
    let mdbook_available = Command::new("mdbook")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    let mut links = Vec::new();
    for book in books {
        let name = book
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid mdBook path '{}'", book.display()))?;
        let output = site.join("books").join(name);
        if mdbook_available {
            let result = Command::new("mdbook")
                .arg("build")
                .arg(book.parent().expect("book has parent"))
                .arg("--dest-dir")
                .arg(&output)
                .output()
                .map_err(|error| format!("could not build mdBook '{name}': {error}"))?;
            if !result.status.success() {
                return Err(format!(
                    "mdBook '{name}' failed: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            links.push((name.to_owned(), format!("books/{name}/index.html")));
        } else {
            let source = book.parent().expect("book has parent").join("src");
            let output = output.join("src");
            copy_tree_if_present(&source, &output)?;
            fs::copy(
                &book,
                output
                    .parent()
                    .expect("book output has parent")
                    .join("book.toml"),
            )
            .map_err(|error| error.to_string())?;
            links.push((name.to_owned(), format!("books/{name}/src/SUMMARY.md")));
        }
    }
    Ok(links)
}

fn copy_tree_if_present(source: &Path, target: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("could not read '{}': {error}", source.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let destination = target.join(entry.file_name());
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            copy_tree_if_present(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination).map_err(|error| error.to_string())?;
        } else {
            return Err(format!(
                "refusing to publish non-regular documentation entry '{}'",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn validate_site_tree(root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(root)
        .map_err(|error| format!("could not inspect docs site '{}': {error}", root.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            validate_site_tree(&entry.path())?;
        } else if !kind.is_file() {
            return Err(format!(
                "refusing to archive non-regular documentation entry '{}'",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn write_site_index(root: &Path, books: &[(String, String)]) -> Result<(), String> {
    let book_links = books
        .iter()
        .map(|(name, path)| {
            format!(
                "<li><a href=\"{}\">{}</a></li>",
                html_escape(path),
                html_escape(name)
            )
        })
        .collect::<String>();
    let contents = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Vapor Documentation</title><h1>Vapor Documentation</h1><h2>Books</h2><ul>{book_links}</ul><h2>Shell guides</h2><ul><li><a href=\"shell/commands.md\">Commands</a></li><li><a href=\"shell/setup.md\">Setup</a></li><li><a href=\"shell/distribution.md\">Distribution</a></li><li><a href=\"shell/steam-development.md\">Steam development</a></li><li><a href=\"shell/discovery.md\">Discovery</a></li></ul><h2>Client</h2><ul><li><a href=\"client/README.md\">README</a></li><li><a href=\"client/App.vapor.toml\">Runtime manifest</a></li><li><a href=\"client/App-Source.vapor.toml\">Source manifest</a></li><li><a href=\"vapor/roadmap/README.md\">Roadmap</a></li></ul>"
    );
    fs::write(root.join("index.html"), contents).map_err(|error| error.to_string())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::{TemporarySite, build_bundle, upload_bundle, upload_url, write_site_index};
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn upload_url_targets_the_docs_archive_endpoint() {
        let url = upload_url("https://vapor.example/docs/").expect("valid endpoint");
        assert_eq!(url.as_str(), "https://vapor.example/docs/current.tar.gz");
    }

    #[test]
    fn upload_url_rejects_cleartext_public_hosts() {
        assert!(upload_url("http://vapor.example/docs").is_err());
    }

    #[test]
    fn upload_url_allows_cleartext_loopback_for_local_server_testing() {
        let url = upload_url("http://127.0.0.1:7112/docs").expect("loopback endpoint");
        assert_eq!(url.as_str(), "http://127.0.0.1:7112/docs/current.tar.gz");
    }

    #[test]
    fn public_bundle_contains_curated_docs_and_excludes_agent_files() {
        let source = TemporarySite::create().expect("temporary source root");
        fs::write(source.path.join("App-Source.vapor.toml"), "schema = 1\n").unwrap();
        fs::write(source.path.join("README.md"), "public readme\n").unwrap();
        let cli_docs = source.path.join("Vapor-CLI/crates/vapor_cli/docs");
        fs::create_dir_all(&cli_docs).unwrap();
        fs::write(cli_docs.join("commands.md"), "public commands\n").unwrap();
        fs::create_dir_all(source.path.join(".agents")).unwrap();
        fs::write(source.path.join(".agents/private.md"), "private\n").unwrap();

        let bundle = build_bundle(&source.path).expect("curated archive");
        let archive_path = source.path.join("site.tar.gz");
        fs::write(&archive_path, bundle).unwrap();
        let listing = std::process::Command::new("tar")
            .arg("-tzf")
            .arg(&archive_path)
            .output()
            .expect("tar can inspect output");
        assert!(listing.status.success());
        let listing = String::from_utf8(listing.stdout).unwrap();
        assert!(listing.lines().any(|path| path == "./index.html"));
        assert!(listing.lines().any(|path| path == "./client/README.md"));
        assert!(listing.lines().any(|path| path == "./shell/commands.md"));
        assert!(!listing.contains(".agents"));
    }

    #[test]
    fn site_index_escapes_book_names_and_links() {
        let site = TemporarySite::create().expect("temporary site");
        write_site_index(
            &site.path,
            &[("<book>".into(), "books/a&b/index.html".into())],
        )
        .expect("write index");
        let index = fs::read_to_string(site.path.join("index.html")).unwrap();
        assert!(index.contains("&lt;book&gt;"));
        assert!(index.contains("a&amp;b"));
    }

    #[test]
    fn upload_sends_the_admin_token_and_archive_to_the_docs_route() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept upload");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 2048];
            loop {
                let count = stream.read(&mut chunk).expect("read upload");
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let header_end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .unwrap()
                        + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or_default();
                    while request.len() < header_end + length {
                        let count = stream.read(&mut chunk).expect("read upload body");
                        request.extend_from_slice(&chunk[..count]);
                    }
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 6\r\nConnection: close\r\n\r\nfiles\n")
                .expect("write response");
            String::from_utf8_lossy(&request).into_owned()
        });
        let url = upload_url(&format!("http://{address}/docs")).expect("loopback URL");
        let response =
            upload_bundle(&url, "test-token", b"archive".to_vec()).expect("upload succeeds");
        assert_eq!(response, "files");
        let request = server.join().expect("server completes");
        assert!(request.starts_with("POST /docs/current.tar.gz HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token")
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/gzip")
        );
        assert!(request.ends_with("archive"));
    }

    #[test]
    fn upload_does_not_follow_redirects_with_the_admin_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
        let address = listener.local_addr().expect("read loopback address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept upload");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).expect("read upload");
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write redirect");
        });
        let url = upload_url(&format!("http://{address}/docs")).expect("loopback URL");
        let error = upload_bundle(&url, "test-token", b"archive".to_vec())
            .expect_err("redirect is not a successful upload");
        assert!(error.contains("302"));
        server.join().expect("server thread");
    }
}
