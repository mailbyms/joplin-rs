//! Minimal WebDAV client: PROPFIND / GET / PUT / DELETE / MKCOL with Basic auth.
//! This is exactly the surface Joplin's file-api-driver-webdav uses, minus the
//! parts only needed for Joplin Cloud / S3.

use crate::{db::base64_encode, error::{Error, Result}, sync::SyncConfig};
use chrono::DateTime;
use quick_xml::{events::Event, reader::Reader};
use reqwest::{
    blocking::{Client, RequestBuilder},
    header::{AUTHORIZATION, CONTENT_TYPE},
    Method, StatusCode,
};
use std::time::Duration;

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:getlastmodified/><d:resourcetype/></d:prop></d:propfind>"#;

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub name: String,
    pub mtime_ms: i64,
    pub is_dir: bool,
}

pub struct WebDav {
    client: Client,
    base: String,
    auth: String,
}

fn err(msg: impl Into<String>) -> Error {
    Error::msg(msg.into())
}

/// Strip a namespace prefix, case-insensitively (`d:href`, `D:HREF`, `href`).
fn local_name(name: &str) -> &str {
    match name.rsplit_once(':') {
        Some((_, local)) => local,
        None => name,
    }
}

fn eq_ignore_ascii(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x.eq_ignore_ascii_case(&y))
}

fn normalize_base(url: &str) -> Result<String> {
    let mut s = url.trim().to_string();
    if s.is_empty() {
        return Err(err("WebDAV 地址不能为空"));
    }
    if !s.starts_with("http://") && !s.starts_with("https://") {
        return Err(err("WebDAV 地址必须以 http:// 或 https:// 开头"));
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    Ok(s)
}

impl WebDav {
    pub fn new(cfg: &SyncConfig) -> Result<Self> {
        let client = Client::builder()
            .danger_accept_invalid_certs(cfg.allow_invalid_certs)
            .timeout(Duration::from_secs(40))
            .connect_timeout(Duration::from_secs(12))
            .build()
            .map_err(|e| err(format!("初始化 HTTP 客户端失败: {e}")))?;
        let auth = format!(
            "Basic {}",
            base64_encode(format!("{}:{}", cfg.username, cfg.password).as_bytes())
        );
        Ok(Self {
            client,
            base: normalize_base(&cfg.url)?,
            auth,
        })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.base, path.trim_start_matches('/'));
        self.client
            .request(method, url)
            .header(AUTHORIZATION, &self.auth)
    }

    fn check(resp: reqwest::blocking::Response, what: &str) -> Result<reqwest::blocking::Response> {
        let status = resp.status();
        if status.is_success() {
            Ok(resp)
        } else {
            let body = resp.text().unwrap_or_default();
            Err(err(format!(
                "{what} 失败: HTTP {status} {}",
                body.chars().take(180).collect::<String>()
            )))
        }
    }

    /// List a collection. `path` is relative to the sync root ("" for root).
    pub fn propfind(&self, path: &str, depth: &str) -> Result<Vec<RemoteFile>> {
        let target = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_matches('/'))
        };
        let resp = self
            .request(Method::from_bytes(b"PROPFIND").unwrap(), &target)
            .header("Depth", depth)
            .header(CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(PROPFIND_BODY)
            .send()
            .map_err(|e| err(format!("PROPFIND 网络错误: {e}")))?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(err(format!(
                "WebDAV 认证失败 (HTTP {status})，请检查用户名/密码"
            )));
        }
        let resp = Self::check(resp, "PROPFIND")?;
        let body = resp.text().map_err(|e| err(format!("读取 PROPFIND 响应失败: {e}")))?;
        parse_propfind(&body)
    }

    pub fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .request(Method::GET, path)
            .send()
            .map_err(|e| err(format!("GET {path} 网络错误: {e}")))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check(resp, &format!("GET {path}"))?;
        Ok(Some(
            resp.bytes()
                .map_err(|e| err(format!("读取 {path} 失败: {e}")))?
                .to_vec(),
        ))
    }

    pub fn get_text(&self, path: &str) -> Result<Option<String>> {
        Ok(self
            .get(path)?
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    pub fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        let resp = self
            .request(Method::PUT, path)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(data.to_vec())
            .send()
            .map_err(|e| err(format!("PUT {path} 网络错误: {e}")))?;
        Self::check(resp, &format!("PUT {path}"))?;
        Ok(())
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        let resp = self
            .request(Method::DELETE, path)
            .send()
            .map_err(|e| err(format!("DELETE {path} 网络错误: {e}")))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::check(resp, &format!("DELETE {path}"))?;
        Ok(())
    }

    pub fn mkcol(&self, path: &str) -> Result<()> {
        let resp = self
            .request(Method::from_bytes(b"MKCOL").unwrap(), &format!("{}/", path.trim_matches('/')))
            .send()
            .map_err(|e| err(format!("MKCOL {path} 网络错误: {e}")))?;
        // 405 = already exists, which is fine.
        if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        Self::check(resp, &format!("MKCOL {path}"))?;
        Ok(())
    }
}

fn parse_propfind(body: &str) -> Result<Vec<RemoteFile>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut href = String::new();
    let mut mtime_raw = String::new();
    let mut is_dir = false;
    let mut in_href = false;
    let mut in_mtime = false;

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| err(format!("解析 PROPFIND XML 失败: {e}")))?
        {
            Event::Eof => break,
            Event::Start(e) => {
                let qname = e.name();
                let n = local_name(qname.as_ref());
                if eq_ignore_ascii(n, "response") {
                    href.clear();
                    mtime_raw.clear();
                    is_dir = false;
                } else if eq_ignore_ascii(n, "href") {
                    in_href = true;
                } else if eq_ignore_ascii(n, "getlastmodified") {
                    in_mtime = true;
                } else if eq_ignore_ascii(n, "collection") {
                    is_dir = true;
                }
            }
            Event::Empty(e) => {
                let qname = e.name();
                if eq_ignore_ascii(local_name(qname.as_ref()), "collection") {
                    is_dir = true;
                }
            }
            Event::Text(t) => {
                // quick-xml 0.42 derefs BytesText to str (already decoded).
                let text = t.as_ref().to_string();
                if in_href {
                    href.push_str(&text);
                } else if in_mtime {
                    mtime_raw.push_str(&text);
                }
            }
            Event::End(e) => {
                let qname = e.name();
                let n = local_name(qname.as_ref());
                if eq_ignore_ascii(n, "href") {
                    in_href = false;
                } else if eq_ignore_ascii(n, "getlastmodified") {
                    in_mtime = false;
                } else if eq_ignore_ascii(n, "response") {
                    let name = href
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        let mtime_ms = DateTime::parse_from_rfc2822(mtime_raw.trim())
                            .or_else(|_| DateTime::parse_from_rfc3339(mtime_raw.trim()))
                            .map(|d| d.timestamp_millis())
                            .unwrap_or(0);
                        out.push(RemoteFile {
                            name,
                            mtime_ms,
                            is_dir,
                        });
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_webdav_multistatus() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/mike/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>
  <D:response><D:href>/mike/a1b2c3d4e5f60718293a4b5c6d7e8f90.md</D:href><D:propstat><D:prop><D:getlastmodified>Sun, 08 Nov 2026 15:46:03 GMT</D:getlastmodified><D:resourcetype/></D:prop></D:propstat></D:response>
</D:multistatus>"#;
        let files = parse_propfind(xml).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].is_dir);
        assert_eq!(files[1].name, "a1b2c3d4e5f60718293a4b5c6d7e8f90.md");
        assert!(files[1].mtime_ms > 0);
    }

    #[test]
    fn normalizes_base_url() {
        assert_eq!(
            normalize_base("https://example.com/notes").unwrap(),
            "https://example.com/notes/"
        );
        assert!(normalize_base("ftp://x").is_err());
    }
}
