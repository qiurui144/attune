use crate::chunker;
use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::parser;
use crate::store::Store;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub depth: u32, // PROPFIND depth: 0=only this resource, 1=children, infinity
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteFile {
    pub href: String,
    pub size: u64,
    pub content_type: String,
    pub last_modified: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteScanResult {
    pub total_files: usize,
    pub new_files: usize,
    pub updated_files: usize,
    pub skipped_files: usize,
    pub errors: Vec<String>,
}

/// List files from WebDAV server via PROPFIND
pub fn list_remote(config: &WebDavConfig) -> Result<Vec<RemoteFile>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| VaultError::LlmUnavailable(format!("client build: {e}")))?;

    let propfind_body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getcontenttype/>
    <D:getlastmodified/>
    <D:displayname/>
  </D:prop>
</D:propfind>"#;

    let method = reqwest::Method::from_bytes(b"PROPFIND")
        .map_err(|e| VaultError::LlmUnavailable(format!("method: {e}")))?;

    let mut req = client
        .request(method, &config.url)
        .header("Depth", config.depth.to_string())
        .header("Content-Type", "application/xml")
        .body(propfind_body);

    if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        req = req.basic_auth(user, Some(pass));
    }

    let resp = req
        .send()
        .map_err(|e| VaultError::LlmUnavailable(format!("webdav request: {e}")))?;

    if !resp.status().is_success() && resp.status().as_u16() != 207 {
        return Err(VaultError::LlmUnavailable(format!(
            "webdav status: {}",
            resp.status()
        )));
    }

    let body = resp
        .text()
        .map_err(|e| VaultError::LlmUnavailable(format!("webdav body: {e}")))?;

    parse_propfind_response(&body)
}

/// Parse multistatus XML (simplified — handles basic Apache/Nginx/Nextcloud output)
fn parse_propfind_response(xml: &str) -> Result<Vec<RemoteFile>> {
    let mut files = Vec::new();
    let mut current_href: Option<String> = None;
    let mut current_size: u64 = 0;
    let mut current_type: String = String::new();
    let mut current_modified: String = String::new();
    let mut is_collection = false;
    let mut in_response = false;

    // Extremely basic XML parsing — sufficient for well-formed WebDAV responses
    let lines: Vec<&str> = xml.split(['<', '>']).collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line == "d:response" || line == "D:response" || line == "response" {
            in_response = true;
            current_href = None;
            current_size = 0;
            current_type = String::new();
            current_modified = String::new();
            is_collection = false;
        } else if (line == "/d:response" || line == "/D:response" || line == "/response")
            && in_response
        {
            if let Some(href) = current_href.take() {
                if !is_collection {
                    files.push(RemoteFile {
                        href,
                        size: current_size,
                        content_type: current_type.clone(),
                        last_modified: current_modified.clone(),
                    });
                }
            }
            in_response = false;
        } else if in_response && (line == "d:href" || line == "D:href" || line == "href") {
            if i + 1 < lines.len() {
                current_href = Some(lines[i + 1].trim().to_string());
            }
        } else if in_response
            && (line == "d:getcontentlength"
                || line == "D:getcontentlength"
                || line == "getcontentlength")
        {
            if i + 1 < lines.len() {
                current_size = lines[i + 1].trim().parse().unwrap_or(0);
            }
        } else if in_response
            && (line == "d:getcontenttype"
                || line == "D:getcontenttype"
                || line == "getcontenttype")
        {
            if i + 1 < lines.len() {
                current_type = lines[i + 1].trim().to_string();
            }
        } else if in_response
            && (line == "d:getlastmodified"
                || line == "D:getlastmodified"
                || line == "getlastmodified")
        {
            if i + 1 < lines.len() {
                current_modified = lines[i + 1].trim().to_string();
            }
        } else if in_response {
            // Handle self-closing collection markers like `<D:collection/>` which split
            // into `D:collection/` with a trailing slash, as well as plain open tags.
            let stripped = line.trim_end_matches('/').trim();
            if stripped == "d:collection"
                || stripped == "D:collection"
                || stripped == "collection"
            {
                is_collection = true;
            }
        }
        i += 1;
    }

    Ok(files)
}

/// Download a remote file (GET)
pub fn fetch_file(config: &WebDavConfig, href: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| VaultError::LlmUnavailable(format!("client build: {e}")))?;

    // Compose URL: if href is absolute use it, else join with base
    let url = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else {
        // Extract origin from config.url
        let base = config.url.split("://").collect::<Vec<_>>();
        if base.len() < 2 {
            return Err(VaultError::LlmUnavailable(format!(
                "invalid base url: {}",
                config.url
            )));
        }
        let scheme = base[0];
        let rest = base[1];
        let host = rest.split('/').next().unwrap_or("");
        format!("{scheme}://{host}{href}")
    };

    let mut req = client.get(&url);
    if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        req = req.basic_auth(user, Some(pass));
    }

    let resp = req
        .send()
        .map_err(|e| VaultError::LlmUnavailable(format!("fetch: {e}")))?;

    if !resp.status().is_success() {
        return Err(VaultError::LlmUnavailable(format!(
            "fetch status: {}",
            resp.status()
        )));
    }

    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| VaultError::LlmUnavailable(format!("fetch body: {e}")))
}

/// Scan remote WebDAV directory: list + download supported files + ingest
pub fn scan_remote(
    config: &WebDavConfig,
    store: &Store,
    dek: &Key32,
    dir_id: &str,
) -> Result<RemoteScanResult> {
    let files = list_remote(config)?;

    let mut result = RemoteScanResult {
        total_files: files.len(),
        new_files: 0,
        updated_files: 0,
        skipped_files: 0,
        errors: vec![],
    };

    let supported_exts = [
        "md", "txt", "py", "js", "ts", "rs", "go", "java", "pdf", "docx",
    ];

    for file in files {
        let filename = file
            .href
            .rsplit('/')
            .next()
            .unwrap_or(&file.href)
            .to_string();
        let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
        if !supported_exts.contains(&ext.as_str()) {
            result.skipped_files += 1;
            continue;
        }

        // Dedup by href (treat as unique path)
        if let Ok(Some(existing)) = store.get_indexed_file(&file.href) {
            if existing.file_hash == file.last_modified {
                result.skipped_files += 1;
                continue;
            }
        }

        match fetch_file(config, &file.href) {
            Ok(bytes) => match parser::parse_bytes(&bytes, &filename) {
                Ok((title, content)) if !content.trim().is_empty() => {
                    match store.insert_item(dek, &title, &content, Some(&file.href), "file", None, None) {
                        Ok(item_id) => {
                            // Enqueue embedding
                            let sections = chunker::extract_sections(&content);
                            let mut chunk_counter = 0;
                            for (section_idx, section_text) in &sections {
                                if !section_text.trim().is_empty() {
                                    let _ = store.enqueue_embedding(
                                        &item_id,
                                        chunk_counter,
                                        section_text,
                                        1,
                                        1,
                                        *section_idx,
                                    );
                                    chunk_counter += 1;
                                }
                            }
                            let _ = store.upsert_indexed_file(
                                dir_id,
                                &file.href,
                                &file.last_modified,
                                &item_id,
                            );
                            result.new_files += 1;
                        }
                        Err(e) => result.errors.push(format!("{filename}: {e}")),
                    }
                }
                Ok(_) => result.skipped_files += 1,
                Err(e) => result.errors.push(format!("{filename}: parse {e}")),
            },
            Err(e) => result.errors.push(format!("{filename}: fetch {e}")),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_propfind() {
        let result = parse_propfind_response("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_propfind_response_basic() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/webdav/file1.md</D:href>
    <D:propstat>
      <D:prop>
        <D:getcontentlength>1234</D:getcontentlength>
        <D:getcontenttype>text/markdown</D:getcontenttype>
        <D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified>
        <D:resourcetype/>
      </D:prop>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let files = parse_propfind_response(xml).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].href, "/webdav/file1.md");
        assert_eq!(files[0].size, 1234);
    }

    #[test]
    fn parse_propfind_skips_collections() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/webdav/</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype><D:collection/></D:resourcetype>
      </D:prop>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/webdav/file.txt</D:href>
    <D:propstat>
      <D:prop>
        <D:getcontentlength>100</D:getcontentlength>
        <D:resourcetype/>
      </D:prop>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let files = parse_propfind_response(xml).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].href, "/webdav/file.txt");
    }
}
