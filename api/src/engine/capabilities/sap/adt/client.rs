use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};

use crate::engine::capabilities::sap::state::SapAdtState;

#[derive(Clone, Debug)]
pub struct AdtHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtReadObjectResult {
    pub object_uri: String,
    pub content_type: Option<String>,
    pub body: String,
}

fn extract_adt_exception_message(xml: &str) -> Option<String> {
    let patterns = [
        "<message lang=\"EN\">",
        "<message>",
        "<localizedMessage lang=\"EN\">",
        "<localizedMessage>",
    ];

    for start_tag in patterns {
        if let Some(start) = xml.find(start_tag) {
            let content_start = start + start_tag.len();
            if let Some(end_rel) = xml[content_start..].find("</") {
                let message = xml[content_start..content_start + end_rel].trim();
                if !message.is_empty() {
                    return Some(message.to_string());
                }
            }
        }
    }

    None
}

fn extract_lock_handle(xml: &str) -> Option<String> {
    let start_tag = "<LOCK_HANDLE>";
    let end_tag = "</LOCK_HANDLE>";
    let start = xml.find(start_tag)? + start_tag.len();
    let end = xml[start..].find(end_tag)? + start;
    let value = xml[start..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn extract_corrnr(xml: &str) -> Option<String> {
    let start_tag = "<CORRNR>";
    let end_tag = "</CORRNR>";
    let start = xml.find(start_tag)? + start_tag.len();
    let end = xml[start..].find(end_tag)? + start;
    let value = xml[start..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub struct AdtUpdateObjectResult {
    pub status: Option<u16>,
    pub body: String,
    pub problems: Vec<String>,
    pub ok: bool,
}

#[derive(Clone)]
pub struct AdtClient {
    state: SapAdtState,
    http: Client,
    csrf_token: Option<String>,
}

impl AdtClient {
    pub fn new(state: &SapAdtState) -> Result<Self> {
        let insecure_tls = std::env::var("SAP_ADT_INSECURE_TLS")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);

        let mut builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::limited(10));

        if insecure_tls {
            eprintln!("[sap_adt] SAP_ADT_INSECURE_TLS enabled; TLS certificate validation is disabled");
            builder = builder.danger_accept_invalid_certs(true);
        }

        let http = builder
            .build()
            .context("failed to build ADT client")?;
        Ok(Self {
            state: state.clone(),
            http,
            csrf_token: None,
        })
    }

    pub fn read_object(&mut self, object_uri: &str, accept: Option<&str>) -> Result<AdtReadObjectResult> {
        let object_uri = normalize_object_uri(object_uri)?;
        let resp = self.request("GET", &object_uri, None, accept.or(Some("text/plain, application/xml, */*")), None)?;
        Ok(AdtReadObjectResult {
            object_uri,
            content_type: response_header(&resp, "content-type"),
            body: resp.body,
        })
    }

    pub fn lock_object(&mut self, object_uri: &str) -> Result<AdtReadObjectResult> {
        let object_uri = normalize_object_uri(object_uri)?;
        let uri = format!("{}{}", object_uri, if object_uri.contains('?') { "&_action=LOCK&accessMode=MODIFY" } else { "?_action=LOCK&accessMode=MODIFY" });
        let accept = if object_uri.contains("/ddic/ddl/sources/") {
            "application/vnd.sap.as+xml;charset=UTF-8;dataname=com.sap.adt.lock.result;q=0.8, application/vnd.sap.as+xml;charset=UTF-8;dataname=com.sap.adt.lock.result2;q=0.9"
        } else {
            "application/xml, text/xml, */*"
        };
        let resp = self.request(
            "POST",
            &uri,
            None,
            Some(accept),
            Some(vec![("X-sap-adt-sessiontype".to_string(), "stateful".to_string())]),
        )?;
        Ok(AdtReadObjectResult {
            object_uri: uri,
            content_type: response_header(&resp, "content-type"),
            body: resp.body,
        })
    }

        pub fn lock_ddl_source(&mut self, object_uri: &str) -> Result<AdtReadObjectResult> {
        let object_uri = normalize_object_uri(object_uri)?;
        let uri = format!("{}{}", object_uri, if object_uri.contains('?') { "&_action=LOCK&accessMode=MODIFY" } else { "?_action=LOCK&accessMode=MODIFY" });
        let resp = self.request(
            "POST",
            &uri,
            None,
            Some("application/vnd.sap.as+xml;charset=UTF-8;dataname=com.sap.adt.lock.result;q=0.8, application/vnd.sap.as+xml;charset=UTF-8;dataname=com.sap.adt.lock.result2;q=0.9"),
            Some(vec![("X-sap-adt-sessiontype".to_string(), "stateful".to_string())]),
        )?;
        Ok(AdtReadObjectResult {
            object_uri: uri,
            content_type: response_header(&resp, "content-type"),
            body: resp.body,
        })
    }

    pub fn format_ddl_identifiers(&mut self, source: &str) -> Result<AdtReadObjectResult> {
        let resp = self.request(
            "POST",
            "/sap/bc/adt/ddic/ddl/formatter/identifiers",
            Some(source.to_string()),
            Some("text/plain"),
            Some(vec![
                ("X-sap-adt-sessiontype".to_string(), "stateful".to_string()),
                ("Content-Type".to_string(), "text/plain".to_string()),
            ]),
        )?;
        Ok(AdtReadObjectResult {
            object_uri: "/sap/bc/adt/ddic/ddl/formatter/identifiers".to_string(),
            content_type: response_header(&resp, "content-type"),
            body: resp.body,
        })
    }

pub fn unlock_object(&mut self, object_uri: &str, lock_handle: &str) -> Result<AdtReadObjectResult> {
        let object_uri = normalize_object_uri(object_uri)?;
        let base_object_uri = object_uri
            .split_once('?')
            .map(|(path, _)| path)
            .unwrap_or(object_uri.as_str())
            .to_string();
        let uri = format!(
            "{}?_action=UNLOCK&lockHandle={}",
            base_object_uri,
            urlencoding::encode(lock_handle.trim())
        );
        let mut headers = vec![("Accept".to_string(), "application/xml, text/xml, */*".to_string())];
        if base_object_uri.contains("/ddic/ddl/sources/") {
            headers.push(("X-sap-adt-sessiontype".to_string(), "stateful".to_string()));
        }
        let resp = self.request("POST", &uri, Some(String::new()), None, Some(headers))?;
        Ok(AdtReadObjectResult {
            object_uri: uri,
            content_type: response_header(&resp, "content-type"),
            body: resp.body,
        })
    }

    pub fn update_object(&mut self, object_uri: &str, source: &str, content_type: Option<&str>, lock_handle: Option<&str>, corr_nr: Option<&str>, extra_headers: Option<Vec<(String, String)>>) -> Result<AdtUpdateObjectResult> {
        let object_uri = normalize_object_uri(object_uri)?;
        let is_ddl_source = object_uri.contains("/ddic/ddl/sources/");
        let mut headers = extra_headers.unwrap_or_default();
        headers.retain(|(key, _)| !key.eq_ignore_ascii_case("if-match"));
        headers.push(("Content-Type".to_string(), content_type.unwrap_or("text/plain; charset=utf-8").to_string()));
        if is_ddl_source {
            headers.push(("X-sap-adt-sessiontype".to_string(), "stateful".to_string()));
            if !headers.iter().any(|(key, _)| key.eq_ignore_ascii_case("accept")) {
                headers.push(("Accept".to_string(), "text/plain".to_string()));
            }
        }

        let mut derived_lock_handle = lock_handle.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let mut derived_corr_nr = corr_nr.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let mut locked_uri: Option<String> = None;

        if derived_lock_handle.is_none() {
            let lock_resp = if is_ddl_source {
                self.lock_ddl_source(&object_uri)?
            } else {
                self.lock_object(&object_uri)?
            };
            derived_lock_handle = extract_lock_handle(&lock_resp.body)
                .or_else(|| extract_tag_value(&lock_resp.body, "LOCK_HANDLE"))
                .or_else(|| extract_tag_value(&lock_resp.body, "lockHandle"));
            derived_corr_nr = extract_corrnr(&lock_resp.body)
                .or_else(|| extract_tag_value(&lock_resp.body, "CORRNR"))
                .or_else(|| extract_tag_value(&lock_resp.body, "corrNr"))
                .or(derived_corr_nr);
            locked_uri = Some(object_uri.clone());
            if derived_lock_handle.is_none() {
                let detail = extract_adt_exception_message(&lock_resp.body)
                    .unwrap_or_else(|| lock_resp.body.trim().to_string());
                if detail.is_empty() {
                    return Err(anyhow!(format!("update_object lock did not return LOCK_HANDLE for {}", object_uri)));
                }
                return Err(anyhow!(format!("update_object lock failed for {}: {}", object_uri, detail)));
            }
        }

        let source_to_write = source.to_string();

        let mut params: Vec<String> = Vec::new();
        if let Some(lock_handle) = derived_lock_handle.as_deref() {
            params.push(format!("lockHandle={}", urlencoding::encode(lock_handle)));
        }
        if let Some(corr_nr) = derived_corr_nr.as_deref() {
            params.push(format!("corrNr={}", urlencoding::encode(corr_nr)));
        }
        let final_uri = if params.is_empty() {
            object_uri.clone()
        } else if object_uri.contains('?') {
            format!("{}&{}", object_uri, params.join("&"))
        } else {
            format!("{}?{}", object_uri, params.join("&"))
        };

        let result = (|| {
            let resp = self.request("PUT", &final_uri, Some(source_to_write), None, Some(headers))?;
            Ok(AdtUpdateObjectResult {
                status: Some(resp.status),
                body: resp.body,
                problems: Vec::new(),
                ok: (200..300).contains(&resp.status),
            })
        })();

        if let Some(lock_handle) = derived_lock_handle.as_deref() {
            let unlock_uri = locked_uri.as_deref().unwrap_or(&object_uri);
            let _ = self.unlock_object(unlock_uri, lock_handle);
        }

        result
    }

    pub fn call_endpoint(&mut self, method: &str, uri: &str, body: Option<&str>, content_type: Option<&str>, accept: Option<&str>, extra_headers: Option<Vec<(String, String)>>) -> Result<AdtHttpResponse> {
        let mut headers = extra_headers.unwrap_or_default();
        if let Some(content_type) = content_type.map(str::trim).filter(|v| !v.is_empty()) {
            headers.push(("Content-Type".to_string(), content_type.to_string()));
        }
        self.request(
            method,
            uri,
            body.map(|v| v.to_string()),
            accept,
            Some(headers),
        )
    }

    fn request(&mut self, method: &str, path_or_url: &str, body: Option<String>, accept: Option<&str>, extra_headers: Option<Vec<(String, String)>>) -> Result<AdtHttpResponse> {
        let requires_csrf = matches!(method, "POST" | "PUT" | "PATCH" | "DELETE");
        if requires_csrf && self.csrf_token.is_none() {
            eprintln!("[sap_adt] csrf_fetch method={} path_or_url={}", method, path_or_url);
            self.fetch_csrf_token()?;
        }

        let resolved_url = self.resolve_url(path_or_url)?;
        log_adt_request(method, &resolved_url, accept, body.as_deref());

        let mut request = self.build_request(method, path_or_url, body.clone(), accept, extra_headers.clone())?;
        if requires_csrf {
            if let Some(token) = self.csrf_token.as_deref() {
                request = request.header("X-CSRF-Token", token);
            }
        }

        let mut response = self.execute(method, &resolved_url, request)?;
        log_adt_response(method, &resolved_url, &response);

        if requires_csrf && response.status == 403 && response.body.to_ascii_lowercase().contains("csrf token validation failed") {
            eprintln!("[sap_adt] csrf_retry method={} url={} status={} body_preview={}", method, resolved_url, response.status, body_preview(&response.body));
            self.csrf_token = None;
            self.fetch_csrf_token()?;
            let mut retry = self.build_request(method, path_or_url, body, accept, extra_headers)?;
            if let Some(token) = self.csrf_token.as_deref() {
                retry = retry.header("X-CSRF-Token", token);
            }
            response = self.execute(method, &resolved_url, retry)?;
            log_adt_response(method, &resolved_url, &response);
        }
        if let Some(token) = response_header(&response, "x-csrf-token") {
            if !token.trim().is_empty() {
                self.csrf_token = Some(token);
            }
        }
        if !(200..300).contains(&response.status) {
            eprintln!("[sap_adt] non_success method={} url={} status={} body_preview={}", method, resolved_url, response.status, body_preview(&response.body));
            if let Some(message) = extract_adt_exception_message(&response.body) {
                return Err(anyhow!(format!("ADT request failed method={} url={} status={}: {}", method, resolved_url, response.status, message)));
            }
        }
        Ok(response)
    }

    fn build_request(&self, method: &str, path_or_url: &str, body: Option<String>, accept: Option<&str>, extra_headers: Option<Vec<(String, String)>>) -> Result<RequestBuilder> {
        let url = self.resolve_url(path_or_url)?;
        let method = reqwest::Method::from_bytes(method.as_bytes()).context("invalid HTTP method")?;
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("mdev-sap-adt/0.1"));
        if let Some(accept) = accept.filter(|v| !v.trim().is_empty()) {
            headers.insert(ACCEPT, HeaderValue::from_str(accept).context("invalid Accept header")?);
        }
        if let Some(client) = self.client_opt() {
            headers.insert(HeaderName::from_static("sap-client"), HeaderValue::from_str(client).context("invalid sap-client header")?);
        }
        self.apply_auth_headers(&mut headers)?;
        for (key, value) in extra_headers.unwrap_or_default() {
            let name = HeaderName::from_bytes(key.trim().as_bytes()).with_context(|| format!("invalid header '{}'", key))?;
            let value = HeaderValue::from_str(value.trim()).with_context(|| format!("invalid header value for '{}'", key))?;
            headers.insert(name, value);
        }
        let mut request = self.http.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.body(body);
        }
        Ok(request)
    }

    fn execute(&self, method: &str, url: &str, request: RequestBuilder) -> Result<AdtHttpResponse> {
        let response = request.send().map_err(|err| {
            let wrapped = anyhow::Error::new(err).context(format!("ADT request failed method={} url={}", method, url));
            log_adt_transport_error(method, url, &wrapped);
            wrapped
        })?;
        self.read_response(response)
    }

    fn read_response(&self, response: Response) -> Result<AdtHttpResponse> {
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or_default().to_string()))
            .collect::<Vec<_>>();
        let body = response.text().context("failed to read ADT response body")?;
        Ok(AdtHttpResponse { status, headers, body })
    }

    fn fetch_csrf_token(&mut self) -> Result<()> {
        let suffix = self.client_opt().map(|client| format!("?sap-client={}", urlencoding::encode(client))).unwrap_or_default();
        let request = self.build_request(
            "GET",
            &format!("/sap/bc/adt/discovery{}", suffix),
            None,
            Some("application/xml, text/xml, */*"),
            Some(vec![("X-CSRF-Token".to_string(), "Fetch".to_string())]),
        )?;
        let url = self.resolve_url(&format!("/sap/bc/adt/discovery{}", suffix))?;
        let response = self.execute("GET", &url, request)?;
        if let Some(token) = response_header(&response, "x-csrf-token") {
            if !token.trim().is_empty() {
                self.csrf_token = Some(token);
            }
        }
        Ok(())
    }

    fn apply_auth_headers(&self, headers: &mut HeaderMap) -> Result<()> {
        match self.auth_type().as_str() {
            "header" => {
                if !self.state.authorization.trim().is_empty() {
                    headers.insert(AUTHORIZATION, HeaderValue::from_str(self.state.authorization.trim()).context("invalid authorization header")?);
                }
            }
            "cookie" => {
                if let Some(cookie_header) = self.state.cookie_header.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                    headers.insert(COOKIE, HeaderValue::from_str(cookie_header).context("invalid cookie header")?);
                }
            }
            _ => {
                if !self.state.username.trim().is_empty() {
                    let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.state.username, self.state.password)));
                    headers.insert(AUTHORIZATION, HeaderValue::from_str(&basic).context("invalid basic auth header")?);
                }
            }
        }
        Ok(())
    }

    fn resolve_url(&self, path_or_url: &str) -> Result<String> {
        let path_or_url = path_or_url.trim();
        if path_or_url.is_empty() {
            bail!("ADT URL is required");
        }
        if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            return Ok(path_or_url.to_string());
        }
        let base = self.state.base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            bail!("SAP ADT base_url is required");
        }
        if path_or_url.starts_with('/') {
            Ok(format!("{}{}", base, path_or_url))
        } else {
            Ok(format!("{}/{}", base, path_or_url))
        }
    }

    fn auth_type(&self) -> String {
        let explicit = self.state.auth_type.trim().to_ascii_lowercase();
        if !explicit.is_empty() {
            return explicit;
        }
        if !self.state.authorization.trim().is_empty() {
            return "header".to_string();
        }
        if self.state.cookie_header.as_deref().map(str::trim).filter(|v| !v.is_empty()).is_some() {
            return "cookie".to_string();
        }
        "basic".to_string()
    }

    fn client_opt(&self) -> Option<&str> {
        let client = self.state.client.trim();
        if client.is_empty() {
            None
        } else {
            Some(client)
        }
    }
}

fn extract_tag_value(body: &str, tag_name: &str) -> Option<String> {
    let open = format!("<{}>", tag_name);
    let close = format!("</{}>", tag_name);
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    let value = body[start..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn normalize_object_uri(object_uri: &str) -> Result<String> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        bail!("object_uri is required");
    }
    Ok(object_uri.to_string())
}

fn response_header(response: &AdtHttpResponse, name: &str) -> Option<String> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn body_preview(body: &str) -> String {
    let normalized = body.replace('\r', " ").replace('\n', " ");
    let trimmed = normalized.trim();
    if trimmed.len() <= 600 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..600])
    }
}

fn log_adt_request(method: &str, url: &str, accept: Option<&str>, body: Option<&str>) {
    eprintln!(
        "[sap_adt] request method={} url={} accept={} body_bytes={} body_preview={}",
        method,
        url,
        accept.unwrap_or_default(),
        body.map(|v| v.len()).unwrap_or(0),
        body.map(body_preview).unwrap_or_default(),
    );
}

fn log_adt_response(method: &str, url: &str, response: &AdtHttpResponse) {
    let content_type = response_header(response, "content-type").unwrap_or_default();
    let csrf = response_header(response, "x-csrf-token").unwrap_or_default();
    eprintln!(
        "[sap_adt] response method={} url={} status={} content_type={} csrf_token={} body_bytes={} body_preview={}",
        method,
        url,
        response.status,
        content_type,
        csrf,
        response.body.len(),
        body_preview(&response.body),
    );
}

fn log_adt_transport_error(method: &str, url: &str, err: &anyhow::Error) {
    eprintln!(
        "[sap_adt] transport_error method={} url={} error={:#}",
        method,
        url,
        err,
    );
}

fn encode_query(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{}={}", urlencoding::encode(key), urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn escape_xml_attr(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
