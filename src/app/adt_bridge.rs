use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, COOKIE, USER_AGENT};
use reqwest::{StatusCode, Url};
use serde_json::{json, Value};

use crate::app::browser_bridge::{self, BrowserTurnConfig};
use crate::app::sap_adt_manifest::{
    SapAdtManifestDocument,
    SapAdtManifestResource,
    SapAdtObjectManifest,
};
use crate::app::state::{
    SapAdtDiscoveryCollection,
    SapAdtDiscoveryState,
    SapAdtObjectSummary,
    SapAdtState,
    SapAdtTemplateLink,
};

#[derive(Clone, Debug)]
pub struct AdtReadObjectResult {
    pub object_uri: String,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtLockObjectResult {
    pub object_uri: String,
    pub lock_handle: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtUpdateObjectResult {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtCheckResult {
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtActivateResult {
    pub body: String,
}

pub fn resolve_relative_object_uri(base_object_uri: &str, child_uri: &str) -> Result<String> {
    let base_object_uri = base_object_uri.trim();
    let child_uri = child_uri.trim();

    if base_object_uri.is_empty() {
        return Err(anyhow!("Base object URI is required"));
    }
    if child_uri.is_empty() {
        return Err(anyhow!("Child object URI is required"));
    }
    if child_uri.starts_with('/') {
        return Ok(child_uri.to_string());
    }
    if child_uri.starts_with("http://") || child_uri.starts_with("https://") {
        let url = Url::parse(child_uri)?;
        let mut uri = url.path().to_string();
        if let Some(query) = url.query() {
            uri.push('?');
            uri.push_str(query);
        }
        return Ok(uri);
    }

    let base = if base_object_uri.starts_with('/') {
        format!("https://dummy{}", base_object_uri)
    } else {
        format!("https://dummy/{}", base_object_uri)
    };
    let joined = Url::parse(&base)?.join(child_uri)?;
    let mut uri = joined.path().to_string();
    if let Some(query) = joined.query() {
        uri.push('?');
        uri.push_str(query);
    }
    Ok(uri)
}

fn resolve_relative_object_uri_as_directory(base_object_uri: &str, child_uri: &str) -> Result<String> {
    let base_object_uri = base_object_uri.trim();
    let child_uri = child_uri.trim();

    if base_object_uri.is_empty() {
        return Err(anyhow!("Base object URI is required"));
    }
    if child_uri.is_empty() {
        return Err(anyhow!("Child object URI is required"));
    }
    if child_uri.starts_with('/') {
        return Ok(child_uri.to_string());
    }
    if child_uri.starts_with("http://") || child_uri.starts_with("https://") {
        let url = Url::parse(child_uri)?;
        let mut uri = url.path().to_string();
        if let Some(query) = url.query() {
            uri.push('?');
            uri.push_str(query);
        }
        return Ok(uri);
    }

    let normalized_base_path = if base_object_uri.ends_with('/') {
        base_object_uri.to_string()
    } else {
        format!("{}/", base_object_uri)
    };

    let base = if normalized_base_path.starts_with('/') {
        format!("https://dummy{}", normalized_base_path)
    } else {
        format!("https://dummy/{}", normalized_base_path)
    };
    let joined = Url::parse(&base)?.join(child_uri)?;
    let mut uri = joined.path().to_string();
    if let Some(query) = joined.query() {
        uri.push('?');
        uri.push_str(query);
    }
    Ok(uri)
}

fn resolve_relative_object_uri_candidates(base_object_uri: &str, child_uri: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();

    let resource_uri = resolve_relative_object_uri(base_object_uri, child_uri)?;
    out.push(resource_uri.clone());

    let directory_uri = resolve_relative_object_uri_as_directory(base_object_uri, child_uri)?;
    if directory_uri != resource_uri {
        out.push(directory_uri);
    }

    Ok(out)
}

pub fn extract_object_source_uri(metadata_uri: &str, metadata_xml: &str) -> Result<Option<String>> {
    if let Some(message) = extract_adt_exception_message(metadata_xml) {
        return Err(anyhow!("ADT object metadata returned exception XML: {}", message));
    }

    let link_re = Regex::new(r#"<(?:(?:[^\s>]+):)?link\b([^>]*)/?>"#)?;
    for caps in link_re.captures_iter(metadata_xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let rel = xml_attr(attrs, "rel");
        let href = xml_attr(attrs, "href");
        let content_type = xml_attr(attrs, "type");

        if rel.as_deref() == Some("http://www.sap.com/adt/relations/source") {
            if let Some(href) = href {
                if !href.trim().is_empty() {
                    if content_type.as_deref() == Some("text/plain") || content_type.is_none() {
                        return Ok(Some(resolve_relative_object_uri(metadata_uri, &href)?));
                    }
                }
            }
        }
    }

    let root_re = Regex::new(r#"<(?:(?:[^\s>]+):)?[^\s>/]+\b([^>]*)>"#)?;
    if let Some(caps) = root_re.captures(metadata_xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(source_uri) = xml_attr(attrs, "abapsource:sourceUri")
            .or_else(|| xml_attr(attrs, "sourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:sourceUri"))
            .or_else(|| xml_attr(attrs, "contentUri"))
            .or_else(|| xml_attr(attrs, "adtcore:contentUri"))
        {
            if !source_uri.trim().is_empty() {
                return Ok(Some(resolve_relative_object_uri(metadata_uri, &source_uri)?));
            }
        }
    }

    Ok(None)
}

fn manifest_slug(value: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }

    let collapsed = out
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if collapsed.is_empty() {
        fallback.to_string()
    } else {
        collapsed
    }
}

fn manifest_header_value(headers: &[(String, String)], key: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn manifest_extension_for(content_type: Option<&str>) -> &'static str {
    let content_type = content_type.unwrap_or_default().to_ascii_lowercase();
    if content_type.contains("abap") {
        "abap"
    } else if content_type.contains("xml") {
        "xml"
    } else if content_type.contains("json") {
        "json"
    } else if content_type.contains("html") {
        "html"
    } else if content_type.contains("javascript") {
        "js"
    } else if content_type.contains("css") {
        "css"
    } else {
        "txt"
    }
}

fn manifest_role_for(rel: &str, title: Option<&str>, content_type: Option<&str>) -> String {
    let rel_lc = rel.to_ascii_lowercase();
    let title_lc = title.unwrap_or_default().to_ascii_lowercase();
    let ct_lc = content_type.unwrap_or_default().to_ascii_lowercase();

    if rel_lc.contains("source") || ct_lc.contains("abap") {
        "source".to_string()
    } else if rel_lc.contains("include") {
        "include".to_string()
    } else if rel_lc.contains("implementation") {
        "implementation".to_string()
    } else if rel_lc.contains("definition") {
        "definition".to_string()
    } else if title_lc.contains("test") {
        "test".to_string()
    } else if ct_lc.contains("xml") {
        "xml".to_string()
    } else {
        "resource".to_string()
    }
}

fn manifest_uri_stem(uri: &str, role: &str) -> Option<String> {
    let mut trimmed = uri.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((before_hash, _)) = trimmed.split_once('#') {
        trimmed = before_hash;
    }

    let has_abap_doc = trimmed
        .split_once('?')
        .map(|(_, query)| query.to_ascii_lowercase().contains("withabapdocfromshorttexts=true"))
        .unwrap_or(false);

    let path_only = trimmed.split_once('?').map(|(path, _)| path).unwrap_or(trimmed);
    let segments = path_only
        .split('/')
        .filter(|segment| !segment.trim().is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        return None;
    }

    let last = manifest_slug(segments[segments.len() - 1], role);
    let mut stem = if segments.len() >= 2 {
        let prev = manifest_slug(segments[segments.len() - 2], role);
        if prev == role || prev == "source" || prev == "sources" || prev == "include" || prev == "includes" {
            format!("{}_{}", prev, last)
        } else {
            last
        }
    } else {
        last
    };

    if has_abap_doc {
        stem = format!("{}_with_abap_doc", stem);
    }

    Some(stem)
}

fn manifest_metadata_declared_stem(metadata_xml: &str, resource_uri: &str, role: &str) -> Option<String> {
    let normalized_resource_uri = resource_uri
        .split_once('#')
        .map(|(path, _)| path)
        .unwrap_or(resource_uri)
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(resource_uri)
        .trim()
        .trim_end_matches('/');

    let tag_re = Regex::new(r#"<(?:(?:[^\s>]+):)?[^\s>/]+\b([^>]*)>"#).ok()?;
    for caps in tag_re.captures_iter(metadata_xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let declared_uri = xml_attr(attrs, "abapsource:sourceUri")
            .or_else(|| xml_attr(attrs, "sourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:sourceUri"))
            .or_else(|| xml_attr(attrs, "contentUri"))
            .or_else(|| xml_attr(attrs, "adtcore:contentUri"));

        let Some(declared_uri) = declared_uri else {
            continue;
        };
        let declared_uri = declared_uri.trim().trim_start_matches("./").trim_end_matches('/');
        if declared_uri.is_empty() {
            continue;
        }
        if !normalized_resource_uri.ends_with(declared_uri) {
            continue;
        }

        if let Some(identity) = xml_attr(attrs, "class:includeType")
            .or_else(|| xml_attr(attrs, "includeType"))
            .or_else(|| xml_attr(attrs, "kind"))
            .or_else(|| xml_attr(attrs, "category"))
            .or_else(|| xml_attr(attrs, "section"))
            .or_else(|| xml_attr(attrs, "part"))
            .filter(|value| !value.trim().is_empty())
        {
            let identity_slug = manifest_slug(&identity, role);
            if let Some(uri_stem) = manifest_uri_stem(declared_uri, role) {
                if uri_stem.ends_with(&identity_slug) {
                    return Some(uri_stem);
                }
            }
            return Some(format!("{}_{}", role, identity_slug));
        }

        if let Some(uri_stem) = manifest_uri_stem(declared_uri, role) {
            return Some(uri_stem);
        }
    }

    None
}

fn manifest_resource_path_for_link(
    metadata_xml: &str,
    index: usize,
    role: &str,
    title: Option<&str>,
    uri: &str,
    content_type: Option<&str>,
) -> String {
    let stem = manifest_metadata_declared_stem(metadata_xml, uri, role)
        .or_else(|| manifest_uri_stem(uri, role))
        .or_else(|| {
            title.map(|value| {
                let slug = manifest_slug(value, role);
                if slug == role {
                    role.to_string()
                } else {
                    format!("{}_{}", role, slug)
                }
            })
        })
        .unwrap_or_else(|| {
            if index == 0 {
                role.to_string()
            } else {
                format!("{}_{}", role, index + 1)
            }
        });

    format!("{}.{}", stem, manifest_extension_for(content_type))
}

fn manifest_resource_path(index: usize, role: &str, title: Option<&str>, content_type: Option<&str>) -> String {
    let stem = if let Some(title) = title {
        let slug = manifest_slug(title, "resource");
        if slug == role {
            role.to_string()
        } else {
            format!("{}_{}", role, slug)
        }
    } else if index == 0 {
        role.to_string()
    } else {
        format!("{}_{}", role, index + 1)
    };

    format!("{}.{}", stem, manifest_extension_for(content_type))
}

fn manifest_document_path(index: usize, title: Option<&str>, content_type: Option<&str>) -> String {
    let stem = title
        .map(|title| manifest_slug(title, "document"))
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| {
            if index == 0 {
                "document".to_string()
            } else {
                format!("document_{}", index + 1)
            }
        });

    format!("{}.{}", stem, manifest_extension_for(content_type))
}

fn metadata_root_attr(metadata_xml: &str, names: &[&str]) -> Option<String> {
    let root_re = Regex::new(r#"<(?:(?:[^\s>]+):)?[^\s>/]+\b([^>]*)>"#).ok()?;
    let caps = root_re.captures(metadata_xml)?;
    let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    for name in names {
        if let Some(value) = xml_attr(attrs, name) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn log_manifest_crawl_step(message: &str) {
    eprintln!("[sap-adt-manifest] {}", message);
}

fn metadata_accept_candidates() -> Vec<&'static str> {
    vec![
        "application/xml, text/xml, */*",
        "text/xml, application/xml, */*",
        "application/vnd.sap.adt.basic.object.properties+xml, application/xml, text/xml, */*",
        "application/vnd.sap.adt.basic.object.properties+xml",
    ]
}

fn is_ui_only_sap_gui_link(rel: &str, title: Option<&str>, content_type: Option<&str>, uri: &str) -> bool {
    let rel_lc = rel.to_ascii_lowercase();
    let title_lc = title.unwrap_or_default().to_ascii_lowercase();
    let ct_lc = content_type.unwrap_or_default().to_ascii_lowercase();
    let uri_lc = uri.to_ascii_lowercase();

    rel_lc.contains("sapgui")
        || rel_lc.contains("gui")
        || title_lc.contains("sap gui")
        || title_lc == "gui"
        || ct_lc.contains("sapgui")
        || ct_lc.contains("sap.gui")
        || uri_lc.contains("sapgui")
        || uri_lc.contains("sap/gui")
}

fn should_skip_manifest_artifact(
    rel: &str,
    title: Option<&str>,
    content_type: Option<&str>,
    uri: &str,
    body: &str,
) -> bool {
    let rel_lc = rel.to_ascii_lowercase();
    let title_lc = title.unwrap_or_default().to_ascii_lowercase();
    let ct_lc = content_type.unwrap_or_default().to_ascii_lowercase();
    let uri_lc = uri.to_ascii_lowercase();
    let body_trimmed = body.trim();

    if body_trimmed.is_empty() {
        return true;
    }

    if rel_lc.contains("source") || title_lc.contains("source") || uri_lc.contains("source") {
        if title_lc.contains("unicode")
            || uri_lc.contains("source_standard_abap_unicode")
            || uri_lc.contains("standard_abap_unicode")
            || rel_lc.contains("standardabapunicode")
            || ct_lc.contains("abap") && body.len() > 200000
        {
            return true;
        }
    }

    false
}

fn read_object_with_accept_fallbacks(
    sap: &mut SapAdtState,
    object_uri: &str,
    accepts: &[&str],
) -> Result<AdtReadObjectResult> {
    let mut errors: Vec<(String, String)> = Vec::new();

    log_manifest_crawl_step(&format!(
        "read_object_with_accept_fallbacks uri={} accepts={}",
        object_uri,
        accepts.join(" | ")
    ));

    for accept in accepts {
        log_manifest_crawl_step(&format!("trying accept={} for uri={}", accept, object_uri));
        match read_object(sap, object_uri, Some(accept)) {
            Ok(result) => {
                log_manifest_crawl_step(&format!(
                    "accept succeeded uri={} accept={} content_type={}",
                    object_uri,
                    accept,
                    result.content_type.clone().unwrap_or_default()
                ));
                return Ok(result);
            }
            Err(err) => {
                let rendered = format!("{:#}", err);
                let retryable = rendered.contains("(406)")
                    || rendered.contains("(415)")
                    || rendered.contains("NotAcceptable")
                    || rendered.contains("Not Acceptable")
                    || rendered.contains("ExceptionResourceNotAcceptable");
                log_manifest_crawl_step(&format!(
                    "accept failed uri={} accept={} retryable={} error={}",
                    object_uri,
                    accept,
                    retryable,
                    rendered
                ));
                errors.push(((*accept).to_string(), rendered.clone()));
                if !retryable {
                    return Err(anyhow!(
                        "read_object failed for {} with Accept={}: {}",
                        object_uri,
                        accept,
                        rendered
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        return Err(anyhow!("No Accept candidates provided for {}", object_uri));
    }

    let attempts = errors
        .into_iter()
        .map(|(accept, err)| format!("Accept={}: {}", accept, err))
        .collect::<Vec<_>>()
        .join(" | ");

    Err(anyhow!(
        "read_object failed for {} after trying metadata Accept fallbacks: {}",
        object_uri,
        attempts
    ))
}

fn read_link_target_with_resolution_fallbacks(
    sap: &mut SapAdtState,
    metadata_uri: &str,
    href: &str,
    accept: Option<&str>,
) -> Result<AdtReadObjectResult> {
    let uri_candidates = resolve_relative_object_uri_candidates(metadata_uri, href)?;
    let mut errors = Vec::new();

    log_manifest_crawl_step(&format!(
        "read_link_target_with_resolution_fallbacks metadata_uri={} href={} candidates={}",
        metadata_uri,
        href,
        uri_candidates.join(" | ")
    ));

    for candidate_uri in uri_candidates {
        if should_skip_link_target(&candidate_uri, None) {
            log_manifest_crawl_step(&format!(
                "link read skipped href={} resolved_uri={}",
                href,
                candidate_uri
            ));
            continue;
        }

        match read_object(sap, &candidate_uri, accept) {
            Ok(result) => {
                log_manifest_crawl_step(&format!(
                    "link read succeeded href={} resolved_uri={} content_type={}",
                    href,
                    candidate_uri,
                    result.content_type.clone().unwrap_or_default()
                ));
                return Ok(result);
            }
            Err(err) => {
                let rendered = format!("{:#}", err);
                log_manifest_crawl_step(&format!(
                    "link read failed href={} resolved_uri={} error={}",
                    href,
                    candidate_uri,
                    rendered
                ));
                errors.push(format!("{} => {}", candidate_uri, rendered));
            }
        }
    }

    Err(anyhow!(
        "read_object failed for href={} from metadata_uri={} after trying resolution candidates: {}",
        href,
        metadata_uri,
        errors.join(" | ")
    ))
}


pub fn manifest_directory_name(
    object_name: Option<&str>,
    object_type: Option<&str>,
    package_name: Option<&str>,
) -> String {
    let package = manifest_slug(package_name.unwrap_or("package"), "package");
    let object_type = manifest_slug(object_type.unwrap_or("object"), "object");
    let object_name = manifest_slug(object_name.unwrap_or("unnamed"), "unnamed");
    format!("sap_adt/{}__{}__{}", package, object_type, object_name)
}

fn log_manifest_summary(manifest: &SapAdtObjectManifest) {
    log_manifest_crawl_step(&format!(
        "manifest metadata_uri={} object_name={} object_type={} package_name={} resources={} documents={}",
        manifest.metadata_uri,
        manifest.object_name.clone().unwrap_or_default(),
        manifest.object_type.clone().unwrap_or_default(),
        manifest.package_name.clone().unwrap_or_default(),
        manifest.resources.len(),
        manifest.documents.len()
    ));
    for resource in &manifest.resources {
        log_manifest_crawl_step(&format!(
            "resource id={} path={} uri={} editable={} readable={} activatable={} content_type={}",
            resource.id,
            resource.path,
            resource.uri,
            resource.editable,
            resource.readable,
            resource.activatable,
            resource.content_type.clone().unwrap_or_default()
        ));
    }
    for document in &manifest.documents {
        log_manifest_crawl_step(&format!(
            "document name={} path={} uri={} content_type={} bytes={}",
            document.title.clone().unwrap_or_default(),
            document.path,
            document.uri,
            document.content_type.clone().unwrap_or_default(),
            document.body.len()
        ));
    }
}

pub fn crawl_object_manifest(
    sap: &mut SapAdtState,
    metadata_uri: &str,
    object_name: Option<&str>,
    object_type: Option<&str>,
    package_name: Option<&str>,
) -> Result<SapAdtObjectManifest> {
    let metadata_uri = metadata_uri.trim();
    if metadata_uri.is_empty() {
        return Err(anyhow!("Metadata URI is required"));
    }

    let metadata_accepts = metadata_accept_candidates();
    let metadata_result = read_object_with_accept_fallbacks(sap, metadata_uri, &metadata_accepts)?;
    log_manifest_crawl_step(&format!(
        "metadata fetched uri={} content_type={} body_bytes={}",
        metadata_uri,
        metadata_result.content_type.clone().unwrap_or_default(),
        metadata_result.body.len()
    ));
    let metadata_xml = metadata_result.body.clone();

    if let Some(message) = extract_adt_exception_message(&metadata_xml) {
        return Err(anyhow!("ADT object metadata returned exception XML: {}", message));
    }

    let mut documents = vec![SapAdtManifestDocument {
        id: "metadata".to_string(),
        uri: metadata_uri.to_string(),
        title: Some("metadata".to_string()),
        content_type: metadata_result
            .content_type
            .clone()
            .or_else(|| Some("application/xml".to_string())),
        headers: metadata_result.headers.clone(),
        path: "metadata.xml".to_string(),
        body: metadata_xml.clone(),
    }];

    let mut resources = Vec::new();
    let mut seen_uris = HashSet::new();
    let link_re = Regex::new(r#"<(?:(?:[^\s>]+):)?link\b([^>]*)/?>"#)?;

    for caps in link_re.captures_iter(&metadata_xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let rel = xml_attr(attrs, "rel").unwrap_or_default();
        let href = match xml_attr(attrs, "href") {
            Some(href) if !href.trim().is_empty() => href,
            _ => continue,
        };
        let uri_candidates = resolve_relative_object_uri_candidates(metadata_uri, &href)?;
        let uri = uri_candidates.first().cloned().unwrap_or_else(|| href.clone());

        let title = xml_attr(attrs, "title");
        let content_type = xml_attr(attrs, "type");
        if is_ui_only_sap_gui_link(&rel, title.as_deref(), content_type.as_deref(), &uri) {
            continue;
        }

        let rel_lc = rel.to_ascii_lowercase();
        let ct_lc = content_type.clone().unwrap_or_default().to_ascii_lowercase();
        let readable = rel_lc.contains("source")
            || rel_lc.contains("implementation")
            || rel_lc.contains("include")
            || rel_lc.contains("definition")
            || ct_lc.starts_with("text/")
            || ct_lc.contains("xml")
            || ct_lc.contains("json")
            || ct_lc.contains("abap");
        let editable = rel_lc.contains("source")
            || rel_lc.contains("implementation")
            || rel_lc.contains("include")
            || rel_lc.contains("definition");
        let activatable = editable;
        let role = manifest_role_for(&rel, title.as_deref(), content_type.as_deref());

        if readable {
            let read_result = if let Some(advertised_content_type) = content_type.as_deref() {
                read_link_target_with_resolution_fallbacks(sap, metadata_uri, &href, Some(advertised_content_type))
                    .or_else(|_| read_link_target_with_resolution_fallbacks(sap, metadata_uri, &href, Some("text/plain, text/*, application/xml, text/xml, application/json, */*")))?
            } else {
                read_link_target_with_resolution_fallbacks(sap, metadata_uri, &href, Some("text/plain, text/*, application/xml, text/xml, application/json, */*"))?
            };
            let resolved_uri = read_result.object_uri.clone();
            if !seen_uris.insert(resolved_uri.clone()) {
                continue;
            }

            if should_skip_manifest_artifact(
                &rel,
                title.as_deref(),
                read_result.content_type.as_deref().or(content_type.as_deref()),
                &resolved_uri,
                &read_result.body,
            ) {
                continue;
            }

            if editable || rel_lc.contains("source") || ct_lc.contains("abap") {
                let path = manifest_resource_path_for_link(
                    &metadata_xml,
                    resources.len(),
                    &role,
                    title.as_deref(),
                    &resolved_uri,
                    read_result.content_type.as_deref().or(content_type.as_deref()),
                );
                resources.push(SapAdtManifestResource {
                    id: format!("resource_{}", resources.len() + 1),
                    uri: resolved_uri,
                    rel: rel.clone(),
                    title: title.clone(),
                    content_type: read_result.content_type.clone().or(content_type.clone()),
                    etag: manifest_header_value(&read_result.headers, "etag"),
                    lock_handle: manifest_header_value(&read_result.headers, "lock_handle")
                        .or_else(|| manifest_header_value(&read_result.headers, "lock-handle"))
                        .or_else(|| manifest_header_value(&read_result.headers, "x-lock-handle"))
                        .or_else(|| manifest_header_value(&read_result.headers, "x-lockhandle")),
                    headers: read_result.headers.clone(),
                    path,
                    readable,
                    editable,
                    activatable,
                    role,
                    body: read_result.body,
                });
            } else {
                let path = manifest_document_path(documents.len(), title.as_deref(), read_result.content_type.as_deref().or(content_type.as_deref()));
                documents.push(SapAdtManifestDocument {
                    id: format!("document_{}", documents.len() + 1),
                    uri: resolved_uri,
                    title: title.clone(),
                    content_type: read_result.content_type.clone().or(content_type.clone()),
                    headers: read_result.headers,
                    path,
                    body: read_result.body,
                });
            }
        } else {
            let mut inserted = false;
            for candidate_uri in uri_candidates {
                if seen_uris.insert(candidate_uri.clone()) {
                    inserted = true;
                    break;
                }
            }
            if !inserted {
                continue;
            }
        }
    }

    if resources.is_empty() {
        log_manifest_crawl_step(&format!(
            "link crawl discovered resources={} documents={} before source fallback",
            resources.len(),
            documents.len()
        ));
        if let Some(source_uri) = extract_object_source_uri(metadata_uri, &metadata_xml)? {
            let read_result = read_link_target_with_resolution_fallbacks(sap, metadata_uri, &source_uri, Some("text/plain, text/*, */*"))?;
            let resolved_uri = read_result.object_uri.clone();
            if seen_uris.insert(resolved_uri.clone()) {
                if !should_skip_manifest_artifact(
                    "http://www.sap.com/adt/relations/source",
                    Some("source"),
                    read_result.content_type.as_deref(),
                    &resolved_uri,
                    &read_result.body,
                ) {
                    resources.push(SapAdtManifestResource {
                        id: "resource_1".to_string(),
                        uri: resolved_uri.clone(),
                        rel: "http://www.sap.com/adt/relations/source".to_string(),
                        title: Some("source".to_string()),
                        content_type: read_result.content_type.clone(),
                        etag: manifest_header_value(&read_result.headers, "etag"),
                        lock_handle: manifest_header_value(&read_result.headers, "lock_handle")
                            .or_else(|| manifest_header_value(&read_result.headers, "lock-handle"))
                            .or_else(|| manifest_header_value(&read_result.headers, "x-lock-handle"))
                            .or_else(|| manifest_header_value(&read_result.headers, "x-lockhandle")),
                        headers: read_result.headers,
                        path: manifest_resource_path_for_link(
                            &metadata_xml,
                            0,
                            "source",
                            Some("source"),
                            &resolved_uri,
                            read_result.content_type.as_deref(),
                        ),
                        readable: true,
                        editable: true,
                        activatable: true,
                        role: "source".to_string(),
                        body: read_result.body,
                    });
                }
            }
        }
    }

    let root_object_name = metadata_root_attr(
        &metadata_xml,
        &["adtcore:name", "abap:name", "name", "objName", "objectName"],
    );
    let root_object_type = metadata_root_attr(
        &metadata_xml,
        &["adtcore:type", "abap:type", "type", "objType", "objectType"],
    );
    let root_package_name = metadata_root_attr(
        &metadata_xml,
        &[
            "adtcore:packageName",
            "abap:packageName",
            "packageName",
            "devclass",
            "adtcore:package",
        ],
    );
    let object_uri = extract_object_source_uri(metadata_uri, &metadata_xml)?
        .and_then(|source_uri| resolve_relative_object_uri_candidates(metadata_uri, &source_uri).ok())
        .and_then(|candidates| candidates.into_iter().find(|candidate| seen_uris.contains(candidate)))
        .or_else(|| resources.first().map(|resource| resource.uri.clone()));
    let manifest_etag = manifest_header_value(&metadata_result.headers, "etag")
        .or_else(|| resources.iter().find_map(|resource| resource.etag.clone()));

    let manifest = SapAdtObjectManifest {
        schema_version: 1,
        metadata_uri: metadata_uri.to_string(),
        object_uri,
        object_name: object_name.map(|v| v.to_string()).or(root_object_name),
        object_type: object_type.map(|v| v.to_string()).or(root_object_type),
        package_name: package_name.map(|v| v.to_string()).or(root_package_name),
        etag: manifest_etag,
        metadata_xml,
        resources,
        documents,
    };
    log_manifest_summary(&manifest);
    Ok(manifest)
}

struct AdtBridgeClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    bridge_dir: Option<String>,
    base_url: Option<String>,
}

impl AdtBridgeClient {
    fn new() -> Self {
        Self {
            child: None,
            stdin: None,
            stdout: None,
            next_id: 1,
            bridge_dir: None,
            base_url: None,
        }
    }

    fn ensure_started(&mut self, bridge_dir: &str, base_url: &str) -> Result<()> {
        if self.child.is_some()
            && self.bridge_dir.as_deref() == Some(bridge_dir)
            && self.base_url.as_deref() == Some(base_url)
        {
            return Ok(());
        }

        self.child = None;
        self.stdin = None;
        self.stdout = None;

        let npm = if cfg!(target_os = "windows") { "npm.cmd" } else { "npm" };
        let mut child = Command::new(npm)
            .arg("start")
            .current_dir(bridge_dir)
            .env("ADT_HOST_URL", base_url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Failed to start ADT bridge from {}", bridge_dir))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("ADT bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("ADT bridge stdout unavailable"))?;

        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.child = Some(child);
        self.bridge_dir = Some(bridge_dir.to_string());
        self.base_url = Some(base_url.to_string());

        std::thread::sleep(Duration::from_millis(1200));
        Ok(())
    }

    fn command_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        format!("adt-{}", id)
    }

    fn send_json(&mut self, mut payload: Value) -> Result<Value> {
        let id = self.command_id();
        payload["id"] = Value::String(id.clone());

        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("ADT bridge stdin not connected"))?;
        writeln!(stdin, "{}", payload).context("Failed writing ADT bridge command")?;
        stdin.flush().context("Failed flushing ADT bridge stdin")?;

        let stdout = self.stdout.as_mut().ok_or_else(|| anyhow!("ADT bridge stdout not connected"))?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).context("Failed reading ADT bridge response")?;
            if n == 0 {
                return Err(anyhow!("ADT bridge exited before sending a response"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if parsed.get("id").and_then(|v| v.as_str()) != Some(id.as_str()) {
                continue;
            }
            if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(parsed);
            }
            let msg = parsed
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("ADT bridge command failed");
            return Err(anyhow!(msg.to_string()));
        }
    }
}

fn bridge_client() -> &'static Mutex<AdtBridgeClient> {
    static CLIENT: OnceLock<Mutex<AdtBridgeClient>> = OnceLock::new();
    CLIENT.get_or_init(|| Mutex::new(AdtBridgeClient::new()))
}

fn transport_bridge_dir(sap: &SapAdtState) -> String {
    for key in ["SAP_ADT_TRANSPORT_BRIDGE_DIR", "ADT_BRIDGE_DIR"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    let browser_dir = std::path::Path::new(&sap.browser_bridge_dir);
    if let Some(parent) = browser_dir.parent() {
        let candidate = parent.join("adt-bridge");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("adt-bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("adt-bridge");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    "adt-bridge".to_string()
}

fn browser_profile_for_adt(sap: &SapAdtState) -> String {
    let profile = sap.browser_channel.trim();
    if profile.is_empty() {
        "msedge".to_string()
    } else {
        profile.to_string()
    }
}

fn normalize_bridge_dir(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn resolve_browser_bridge_dir(sap: &SapAdtState) -> String {
    let normalized = normalize_bridge_dir(&sap.browser_bridge_dir);
    if !normalized.is_empty() && std::path::Path::new(&normalized).is_dir() {
        return normalized;
    }

    let browser_dir = std::path::Path::new(&normalized);
    if let Some(parent) = browser_dir.parent() {
        let candidate = parent.join("adt-bridge");
        if candidate.is_dir() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("adt-bridge");
            if candidate.is_dir() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("adt-bridge");
        if candidate.is_dir() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    normalized
}

fn browser_cfg_from_state(sap: &SapAdtState) -> BrowserTurnConfig {
    BrowserTurnConfig {
        bridge_dir: resolve_browser_bridge_dir(sap),
        edge_executable: if cfg!(target_os = "windows") {
            "msedge".to_string()
        } else {
            "msedge".to_string()
        },
        user_data_dir: sap.browser_user_data_dir.clone(),
        cdp_url: "http://127.0.0.1:9222".to_string(),
        page_url_contains: "sap".to_string(),
        profile: browser_profile_for_adt(sap),
        session_id: sap.browser_session_id.clone(),
        auto_launch_edge: false,
        runtime_key: "sap_adt".to_string(),
        response_timeout_ms: 60_000,
        response_poll_ms: 1_000,
        dom_poll_ms: 1_000,
    }
}

fn require_discovery_url(sap: &SapAdtState) -> Result<String> {
    sap.discovery_url
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("SAP ADT discovery URL is not set"))
}

fn validate_bridge_settings(sap: &SapAdtState) -> Result<()> {
    let resolved_bridge_dir = resolve_browser_bridge_dir(sap);
    if resolved_bridge_dir.trim().is_empty() {
        return Err(anyhow!("SAP ADT browser bridge directory is not set"));
    }
    if !std::path::Path::new(&resolved_bridge_dir).is_dir() {
        return Err(anyhow!(format!("SAP ADT browser bridge directory does not exist: {}", resolved_bridge_dir)));
    }
    if sap.browser_user_data_dir.trim().is_empty() {
        return Err(anyhow!("SAP ADT browser user data dir is not set"));
    }
    Ok(())
}

fn cookie_urls_for_discovery(discovery_url: &str) -> Vec<String> {
    let mut urls = vec![discovery_url.to_string()];
    if let Ok(url) = Url::parse(discovery_url) {
        let origin = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port().map(|p| format!(":{}", p)).unwrap_or_default()
        );
        if !urls.iter().any(|u| u == &origin) {
            urls.push(origin);
        }
    }
    urls
}

fn refresh_cookie_header(sap: &mut SapAdtState, discovery_url: &str) -> Result<String> {
    validate_bridge_settings(sap)?;

    let mut cfg = browser_cfg_from_state(sap);
    if cfg.session_id.is_none() {
        browser_bridge::launch_browser(&mut cfg, Some(discovery_url))
            .context("Failed to launch browser bridge session for SAP ADT")?;
    }

    let urls = cookie_urls_for_discovery(discovery_url);
    let cookie_header = browser_bridge::get_session_cookies(&mut cfg, &urls)
        .context("Failed to harvest SAP ADT cookies from browser bridge")?;

    sap.browser_session_id = cfg.session_id.clone();
    sap.cookie_header = Some(cookie_header.clone());
    Ok(cookie_header)
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(60))
        .build()
        .context("Failed to build SAP ADT HTTP client")
}

fn send_discovery_request(client: &Client, discovery_url: &str, cookie_header: &str) -> Result<(StatusCode, String)> {
    let response = client
        .get(discovery_url)
        .header(COOKIE, cookie_header)
        .header(ACCEPT, "application/xml, text/xml, */*")
        .header(USER_AGENT, "mdev-sap-adt/1.0")
        .send()
        .with_context(|| format!("Failed to send SAP ADT discovery request to {}", discovery_url))?;

    let status = response.status();
    let body = response.text().unwrap_or_default();
    Ok((status, body))
}

fn xml_decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn xml_attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{}=\"", name);
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(xml_decode(&rest[..end]))
}

fn first_tag_text(block: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"(?s)<{}\b[^>]*>(.*?)</{}>", regex::escape(tag), regex::escape(tag));
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(block)?;
    Some(xml_decode(caps.get(1)?.as_str().trim()))
}

fn collect_tag_texts(block: &str, tag: &str) -> Vec<String> {
    let pattern = format!(r"(?s)<{}\b[^>]*>(.*?)</{}>", regex::escape(tag), regex::escape(tag));
    let Ok(re) = Regex::new(&pattern) else {
        return vec![];
    };

    re.captures_iter(block)
        .filter_map(|caps| caps.get(1).map(|m| xml_decode(m.as_str().trim())))
        .filter(|s| !s.is_empty())
        .collect()
}

fn compact_xml_preview(xml: &str, limit: usize) -> String {
    let compact = xml.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(limit).collect()
}

fn extract_adt_exception_message(xml: &str) -> Option<String> {
    let type_re = Regex::new(r#"<type\b[^>]*id=\"([^\"]+)\"[^>]*/?>"#).ok()?;
    let message_re = Regex::new(r#"(?s)<(?:[^\s>]+:)?(?:localizedMessage|message)\b[^>]*>(.*?)</(?:[^\s>]+:)?(?:localizedMessage|message)>"#).ok()?;

    let type_id = type_re
        .captures(xml)
        .and_then(|caps| caps.get(1).map(|m| xml_decode(m.as_str())));

    let message = message_re
        .captures_iter(xml)
        .filter_map(|caps| caps.get(1).map(|m| xml_decode(m.as_str().trim())))
        .find(|s| !s.is_empty());

    if type_id.is_none() && message.is_none() {
        return None;
    }

    Some(match (type_id, message) {
        (Some(t), Some(m)) => format!("{}: {}", t, m),
        (Some(t), None) => t,
        (None, Some(m)) => m,
        (None, None) => String::new(),
    })
}

fn build_package_search_queries(package_name: &str) -> Vec<String> {
    let trimmed = package_name.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut queries = Vec::new();
    queries.push(format!("{}*", trimmed));

    if let Some((stem, _)) = trimmed.rsplit_once('_') {
        let stem = stem.trim();
        if !stem.is_empty() {
            queries.push(format!("{}*", stem));
        }
    }

    queries.push(trimmed.to_string());
    queries.dedup();
    queries
}

fn parse_discovery(xml: &str) -> Result<SapAdtDiscoveryState> {
    let workspace_re = Regex::new(r"(?s)<app:workspace\b[^>]*>(.*?)</app:workspace>")?;
    let collection_re = Regex::new(r"(?s)<app:collection\b([^>]*)>(.*?)</app:collection>")?;
    let category_re = Regex::new(r#"<atom:category\b([^>]*)/?>"#)?;
    let template_link_re = Regex::new(r#"<adtcomp:templateLink\b([^>]*)/?>"#)?;

    let mut workspaces = Vec::new();
    for caps in workspace_re.captures_iter(xml) {
        let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(title) = first_tag_text(body, "atom:title") {
            if !title.is_empty() {
                workspaces.push(title);
            }
        }
    }

    let mut collections = Vec::new();
    for caps in collection_re.captures_iter(xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let href = xml_attr(attrs, "href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }

        let title = first_tag_text(body, "atom:title").unwrap_or_else(|| href.clone());
        let accepts = collect_tag_texts(body, "app:accept");

        let category_caps = category_re.captures(body);
        let (category_term, category_scheme) = if let Some(cat_caps) = category_caps {
            let cat_attrs = cat_caps.get(1).map(|m| m.as_str()).unwrap_or("");
            (xml_attr(cat_attrs, "term"), xml_attr(cat_attrs, "scheme"))
        } else {
            (None, None)
        };

        let template_links = template_link_re
            .captures_iter(body)
            .map(|tpl_caps| {
                let tpl_attrs = tpl_caps.get(1).map(|m| m.as_str()).unwrap_or("");
                SapAdtTemplateLink {
                    rel: xml_attr(tpl_attrs, "rel").unwrap_or_default(),
                    template: xml_attr(tpl_attrs, "template").unwrap_or_default(),
                    title: xml_attr(tpl_attrs, "title"),
                }
            })
            .filter(|tpl| !tpl.template.is_empty() || !tpl.rel.is_empty())
            .collect::<Vec<_>>();

        collections.push(SapAdtDiscoveryCollection {
            title,
            href,
            category_term,
            category_scheme,
            accepts,
            template_links,
        });
    }

    if collections.is_empty() {
        return Err(anyhow!("SAP ADT discovery XML contained no collections"));
    }

    let package_collection_href = collections
        .iter()
        .find(|c| c.href == "/sap/bc/adt/packages")
        .map(|c| c.href.clone());

    let package_tree_href = package_collection_href
        .as_ref()
        .map(|href| format!("{}/$tree", href.trim_end_matches('/')));

    let repository_search_collection = collections
        .iter()
        .find(|c| c.href == "/sap/bc/adt/repository/informationsystem/search");

    let repository_search_href = repository_search_collection.map(|c| c.href.clone());
    let repository_search_template = repository_search_collection
        .and_then(|c| {
            c.template_links
                .iter()
                .find(|tpl| tpl.template.contains("/sap/bc/adt/repository/informationsystem/search"))
                .map(|tpl| tpl.template.clone())
        })
        .or_else(|| repository_search_href.clone());

    let object_types_href = collections
        .iter()
        .find(|c| c.href == "/sap/bc/adt/repository/informationsystem/objecttypes")
        .map(|c| c.href.clone());

    let enabled = package_tree_href.is_some() && repository_search_template.is_some();

    Ok(SapAdtDiscoveryState {
        workspaces,
        collections,
        package_collection_href,
        package_tree_href,
        repository_search_href,
        repository_search_template,
        object_types_href,
        enabled,
    })
}

fn bridge_base_url(discovery_url: &str) -> Result<String> {
    let url = Url::parse(discovery_url)
        .with_context(|| format!("Invalid SAP ADT discovery URL: {}", discovery_url))?;
    let host = url.host_str().ok_or_else(|| anyhow!("SAP ADT discovery URL missing host"))?;
    let mut out = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    Ok(out)
}

fn connect_transport_session(sap: &mut SapAdtState, cookie_header: &str) -> Result<String> {
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "connect",
        "session_id": sap.adt_session_id.clone(),
        "base_url": base_url,
        "auth_type": "cookie",
        "cookie_header": cookie_header,
        "timeout_ms": 60000
    }))?;

    let session_id = resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| resp.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow!("ADT connect response missing session_id"))?
        .to_string();

    sap.adt_session_id = Some(session_id.clone());
    Ok(session_id)
}

fn ensure_transport_session(sap: &mut SapAdtState) -> Result<String> {
    if let Some(session_id) = sap.adt_session_id.clone() {
        if !session_id.trim().is_empty() {
            return Ok(session_id);
        }
    }

    let cookie_header = sap
        .cookie_header
        .clone()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow!("SAP ADT cookie header is not available"))?;

    connect_transport_session(sap, &cookie_header)
}

fn xml_debug_tag_samples(xml: &str, limit: usize) -> Vec<String> {
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() && out.len() < limit {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }

        let start = i + 1;
        if start >= bytes.len() {
            break;
        }

        let first = bytes[start];
        if first == b'/' || first == b'!' || first == b'?' {
            i += 1;
            continue;
        }

        let mut end = start;
        while end < bytes.len() {
            let ch = bytes[end];
            if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' || ch == b'>' || ch == b'/' {
                break;
            }
            end += 1;
        }

        if end > start {
            let tag = &xml[start..end];
            if !out.iter().any(|existing| existing == tag) {
                out.push(tag.to_string());
            }
        }

        i = end;
    }

    out
}

fn xml_debug_excerpt_lines(xml: &str, limit: usize) -> Vec<String> {
    xml.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(limit)
        .map(|line| {
            if line.len() > 220 {
                format!("{}...", &line[..220])
            } else {
                line.to_string()
            }
        })
        .collect()
}

fn xml_tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
}

fn log_matching_asxml_repository_node(xml: &str, package_name: &str, package_uri: &str) {
    let open = "<SEU_ADT_REPOSITORY_OBJ_NODE>";
    let close = "</SEU_ADT_REPOSITORY_OBJ_NODE>";
    let mut cursor = 0usize;
    let package_name_upper = package_name.trim().to_ascii_uppercase();
    let package_uri_lower = package_uri.trim().to_ascii_lowercase();

    while let Some(start_rel) = xml[cursor..].find(open) {
        let start = cursor + start_rel + open.len();
        let rest = &xml[start..];
        let Some(end_rel) = rest.find(close) else {
            break;
        };
        let block = &rest[..end_rel];

        let object_name = xml_tag_text(block, "OBJECT_NAME").unwrap_or_default();
        let tech_name = xml_tag_text(block, "TECH_NAME").unwrap_or_default();
        let object_uri = xml_tag_text(block, "OBJECT_URI").unwrap_or_default();

        let matches_name = object_name.trim().eq_ignore_ascii_case(&package_name_upper)
            || tech_name.trim().eq_ignore_ascii_case(&package_name_upper);
        let matches_uri = object_uri.trim().eq_ignore_ascii_case(&package_uri_lower);

        if matches_name || matches_uri {
            let node_id = xml_tag_text(block, "NODE_ID").unwrap_or_default();
            let parent_name = xml_tag_text(block, "PARENT_NAME").unwrap_or_default();
            let expandable = xml_tag_text(block, "EXPANDABLE").unwrap_or_default();
            let object_type = xml_tag_text(block, "OBJECT_TYPE").unwrap_or_default();
            let object_vit_uri = xml_tag_text(block, "OBJECT_VIT_URI").unwrap_or_default();
            let description = xml_tag_text(block, "DESCRIPTION").unwrap_or_default();
            eprintln!(
                "[sap_adt] matching package node package={} object_type={} object_name={} tech_name={} object_uri={} object_vit_uri={} node_id={} parent_name={} expandable={} description={} block={}",
                package_name,
                object_type,
                object_name,
                tech_name,
                object_uri,
                object_vit_uri,
                node_id,
                parent_name,
                expandable,
                description,
                block
            );
            return;
        }

        cursor = start + end_rel + close.len();
    }

    eprintln!(
        "[sap_adt] matching package node not found package={} package_uri={}",
        package_name,
        package_uri
    );
}

fn looks_like_generated_include_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    let eq_count = trimmed.chars().filter(|c| *c == '=').count();
    let mostly_equals = eq_count >= 8;
    let ends_like_classpool = trimmed.ends_with("CP") || trimmed.ends_with("CCAU") || trimmed.ends_with("CM") || trimmed.ends_with("CU");
    mostly_equals || ends_like_classpool
}

fn choose_asxml_node_name(object_type: &str, uri: &str, object_name: &str, tech_name: &str, description: &str) -> String {
    let is_oo_uri = uri.starts_with("/sap/bc/adt/oo/")
        || uri.starts_with("/sap/bc/adt/classes/")
        || uri.starts_with("/sap/bc/adt/interfaces/");
    let is_oo_type = object_type.starts_with("CLAS/") || object_type.starts_with("INTF/");

    let object_name = object_name.trim();
    let tech_name = tech_name.trim();
    let description = description.trim();
    let uri_leaf = uri.rsplit('/').next().unwrap_or(uri).trim();

    if is_oo_uri || is_oo_type {
        if !tech_name.is_empty() && !looks_like_generated_include_name(tech_name) {
            return tech_name.to_string();
        }
        if !object_name.is_empty() && !looks_like_generated_include_name(object_name) {
            return object_name.to_string();
        }
        if !uri_leaf.is_empty() {
            return uri_leaf.to_ascii_uppercase();
        }
        if !description.is_empty() {
            return description.to_string();
        }
    }

    if !object_name.is_empty() && !looks_like_generated_include_name(object_name) {
        return object_name.to_string();
    }
    if !tech_name.is_empty() && !looks_like_generated_include_name(tech_name) {
        return tech_name.to_string();
    }
    if !description.is_empty() {
        return description.to_string();
    }

    uri_leaf.to_string()
}

fn looks_like_generated_oo_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    let eq_count = trimmed.chars().filter(|c| *c == '=').count();
    eq_count >= 8 || trimmed.ends_with("CP") || trimmed.ends_with("CM") || trimmed.ends_with("CU") || trimmed.ends_with("CCAU")
}

fn choose_asxml_display_name(object_type: &str, uri: &str, object_name: &str, tech_name: &str, description: &str) -> String {
    let object_name = object_name.trim();
    let tech_name = tech_name.trim();
    let description = description.trim();

    let is_oo_object = object_type.starts_with("CLAS/")
        || object_type.starts_with("INTF/")
        || uri.starts_with("/sap/bc/adt/oo/classes/")
        || uri.starts_with("/sap/bc/adt/oo/interfaces/");

    if is_oo_object {
        if !tech_name.is_empty() && !looks_like_generated_oo_name(tech_name) {
            return tech_name.to_string();
        }
        if !object_name.is_empty() && !looks_like_generated_oo_name(object_name) {
            return object_name.to_string();
        }
        if !description.is_empty() {
            return description.to_string();
        }
    }

    if !tech_name.is_empty() {
        return tech_name.to_string();
    }
    if !object_name.is_empty() {
        return object_name.to_string();
    }
    if !description.is_empty() {
        return description.to_string();
    }

    uri.rsplit('/').next().unwrap_or(uri).to_string()
}

fn parse_asxml_repository_nodes(xml: &str) -> Vec<SapAdtObjectSummary> {
    let mut out = Vec::new();
    let open = "<SEU_ADT_REPOSITORY_OBJ_NODE>";
    let close = "</SEU_ADT_REPOSITORY_OBJ_NODE>";
    let mut cursor = 0usize;

    while let Some(start_rel) = xml[cursor..].find(open) {
        let start = cursor + start_rel + open.len();
        let rest = &xml[start..];
        let Some(end_rel) = rest.find(close) else {
            break;
        };
        let block = &rest[..end_rel];

        let object_type = xml_tag_text(block, "OBJECT_TYPE")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "OBJECT".to_string());

        let uri = xml_tag_text(block, "OBJECT_URI")
            .or_else(|| xml_tag_text(block, "OBJECT_VIT_URI"))
            .unwrap_or_default();

        if !uri.is_empty() {
            let object_name = xml_tag_text(block, "OBJECT_NAME").unwrap_or_default();
            let tech_name = xml_tag_text(block, "TECH_NAME").unwrap_or_default();
            let raw_description = xml_tag_text(block, "DESCRIPTION").unwrap_or_default();

            let name = choose_asxml_node_name(
                &object_type,
                &uri,
                &object_name,
                &tech_name,
                &raw_description
            );

            let description = if raw_description.trim().is_empty() {
                name.clone()
            } else {
                raw_description
            };

            let is_structural_node = object_type.starts_with("DEVC/")
                && !uri.starts_with("/sap/bc/adt/programs/")
                && !uri.starts_with("/sap/bc/adt/oo/")
                && !uri.starts_with("/sap/bc/adt/ddic/")
                && !uri.starts_with("/sap/bc/adt/ddls/")
                && !uri.starts_with("/sap/bc/adt/cds/")
                && !uri.starts_with("/sap/bc/adt/classes/")
                && !uri.starts_with("/sap/bc/adt/interfaces/")
                && !uri.starts_with("/sap/bc/adt/functions/");

            out.push(SapAdtObjectSummary {
                name,
                object_type,
                package_name: None,
                uri: uri.clone(),
                source_uri: if is_structural_node { None } else { Some(uri) },
                description: Some(description),
            });
        }

        cursor = start + end_rel + close.len();
    }

    out
}

pub fn parse_package_tree_xml(xml: &str) -> Result<Vec<SapAdtObjectSummary>> {
    if let Some(message) = extract_adt_exception_message(xml) {
        return Err(anyhow!("ADT repository search returned exception XML: {}", message));
    }

    if let Some(message) = extract_adt_exception_message(xml) {
        return Err(anyhow!("ADT package tree returned exception XML: {}", message));
    }

    let item_re = Regex::new(r#"<(?:(?:[^\s>]+):)?(?:objectReference|treeNode|node|objectNode)\b([^>]*)/?>"#)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for item in parse_asxml_repository_nodes(xml) {
        if item.uri.trim().is_empty() || !seen.insert(item.uri.clone()) {
            continue;
        }
        out.push(item);
    }

    if !out.is_empty() {
        out.sort_by(|a, b| {
            a.object_type
                .to_lowercase()
                .cmp(&b.object_type.to_lowercase())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.uri.cmp(&b.uri))
        });

        let preview: Vec<String> = out
            .iter()
            .take(20)
            .map(|item| format!("{} {} {}", item.object_type, item.name, item.uri))
            .collect();
        eprintln!(
            "[sap_adt] parse_package_tree_xml recognized={} preview={}",
            out.len(),
            preview.join(" | ")
        );

        return Ok(out);
    }

    for caps in item_re.captures_iter(xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        let uri = xml_attr(attrs, "adtcore:uri")
            .or_else(|| xml_attr(attrs, "uri"))
            .or_else(|| xml_attr(attrs, "objectUri"))
            .or_else(|| xml_attr(attrs, "refUri"))
            .or_else(|| xml_attr(attrs, "resourceUri"))
            .or_else(|| xml_attr(attrs, "href"));
        let Some(uri) = uri else {
            continue;
        };
        if uri.trim().is_empty() || !seen.insert(uri.clone()) {
            continue;
        }

        let source_uri = xml_attr(attrs, "adtcore:sourceUri")
            .or_else(|| xml_attr(attrs, "sourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:sourceResourceUri"))
            .or_else(|| xml_attr(attrs, "sourceResourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:contentUri"))
            .or_else(|| xml_attr(attrs, "contentUri"));

        let name = xml_attr(attrs, "adtcore:name")
            .or_else(|| xml_attr(attrs, "name"))
            .or_else(|| xml_attr(attrs, "techName"))
            .or_else(|| xml_attr(attrs, "displayName"))
            .or_else(|| xml_attr(attrs, "shortDescription"))
            .or_else(|| xml_attr(attrs, "label"))
            .or_else(|| xml_attr(attrs, "title"))
            .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or(uri.as_str()).to_string());

        let object_type = xml_attr(attrs, "adtcore:type")
            .or_else(|| xml_attr(attrs, "type"))
            .or_else(|| xml_attr(attrs, "objectType"))
            .or_else(|| xml_attr(attrs, "nodeType"))
            .or_else(|| xml_attr(attrs, "adtcore:category"))
            .unwrap_or_else(|| "OBJECT".to_string());

        let package_name = xml_attr(attrs, "adtcore:packageName")
            .or_else(|| xml_attr(attrs, "packageName"));

        let description = xml_attr(attrs, "adtcore:description")
            .or_else(|| xml_attr(attrs, "description"))
            .or_else(|| xml_attr(attrs, "title"));

        out.push(SapAdtObjectSummary {
            uri,
            source_uri,
            name,
            object_type,
            package_name,
            description,
        });
    }

    out.sort_by(|a, b| {
        a.object_type
            .to_lowercase()
            .cmp(&b.object_type.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.uri.cmp(&b.uri))
    });

    if out.is_empty() {
        let tags = xml_debug_tag_samples(xml, 40);
        let lines = xml_debug_excerpt_lines(xml, 20);
        eprintln!(
            "[sap_adt] parse_package_tree_xml recognized=0 xml_bytes={} tag_samples={}",
            xml.len(),
            tags.join(" | ")
        );
        for (idx, line) in lines.iter().enumerate() {
            eprintln!("[sap_adt] parse_package_tree_xml line[{}]={}", idx, line);
        }
    } else {
        let preview: Vec<String> = out
            .iter()
            .take(20)
            .map(|item| format!("{} {} {}", item.object_type, item.name, item.uri))
            .collect();
        eprintln!(
            "[sap_adt] parse_package_tree_xml recognized={} preview={}",
            out.len(),
            preview.join(" | ")
        );
    }

    Ok(out)
}

pub fn connect(sap: &mut SapAdtState) -> Result<()> {
    let discovery_url = require_discovery_url(sap)?;

    sap.connected = false;
    sap.last_error = None;
    sap.last_status = Some("Refreshing SAP ADT authentication".to_string());

    let mut cookie_header = sap.cookie_header.clone().unwrap_or_default();
    if cookie_header.trim().is_empty() {
        cookie_header = refresh_cookie_header(sap, &discovery_url)?;
    }

    let client = build_http_client()?;
    let (status, body) = send_discovery_request(&client, &discovery_url, &cookie_header)?;

    let final_body = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let refreshed_cookie_header = refresh_cookie_header(sap, &discovery_url)?;
        let (retry_status, retry_body) = send_discovery_request(&client, &discovery_url, &refreshed_cookie_header)?;
        if !retry_status.is_success() {
            return Err(anyhow!(
                "SAP ADT discovery returned {} after cookie refresh: {}",
                retry_status,
                retry_body
            ));
        }
        retry_body
    } else if !status.is_success() {
        return Err(anyhow!("SAP ADT discovery returned {}: {}", status, body));
    } else {
        body
    };

    let discovery = parse_discovery(&final_body)?;
    if !discovery.enabled {
        return Err(anyhow!(
            "SAP ADT discovery was retrieved but did not expose the package tree and repository search metadata needed by mdev"
        ));
    }

    let session_id = connect_transport_session(sap, &cookie_header)?;
    let workspace_count = discovery.workspaces.len();
    let collection_count = discovery.collections.len();

    sap.connected = true;
    sap.discovery_xml = final_body;
    sap.discovery = Some(discovery);
    sap.adt_session_id = Some(session_id);
    sap.last_error = None;
    sap.last_status = Some(format!(
        "Connected: discovery ingested ({} workspaces, {} collections)",
        workspace_count,
        collection_count
    ));

    Ok(())
}

fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

fn package_tree_attempts(_include_subpackages: bool) -> Vec<Vec<(&'static str, &'static str)>> {
    vec![vec![]]
}

fn looks_like_empty_package_tree(xml: &str) -> bool {
    let trimmed = xml.trim();
    let lower = trimmed.to_ascii_lowercase();
    let has_package_tree = lower.contains("<packagetree") || lower.contains(":packagetree");
    let has_object_reference = lower.contains("<objectreference") || lower.contains(":objectreference");
    let has_uri_attr = lower.contains(" uri=") || lower.contains(" adtcore:uri=") || lower.contains("\nuri=") || lower.contains("\nadtcore:uri=");
    has_package_tree && !has_object_reference && !has_uri_attr
}

fn load_package_tree_xml(


    client: &mut AdtBridgeClient,
    session_id: &str,
    package_name: &str,
    include_subpackages: bool,
) -> Result<String> {
    let mut last_xml = String::new();

    for attempt in package_tree_attempts(include_subpackages) {
        let mut query = format!("packagename={}", encode_component(package_name));
        for (key, value) in attempt {
            query.push('&');
            query.push_str(key);
            query.push('=');
            query.push_str(&encode_component(value));
        }
        let uri = format!("/sap/bc/adt/packages/$tree?{}", query);

        let resp = client.send_json(json!({
            "cmd": "call_endpoint",
            "session_id": session_id,
            "method": "GET",
            "uri": uri,
            "accept": "application/xml, text/xml, */*"
        }))?;

        let xml = resp
            .get("data")
            .and_then(|v| v.get("body").or_else(|| v.get("xml")))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ADT package tree response missing body"))?
            .to_string();

        let preview = compact_xml_preview(&xml, 400);
        eprintln!(
            "[sap_adt] package tree package={} include_subpackages={} xml_bytes={} xml_preview={}",
            package_name,
            include_subpackages,
            xml.len(),
            preview
        );

        if let Some(message) = extract_adt_exception_message(&xml) {
            eprintln!(
                "[sap_adt] package tree exception package={} include_subpackages={} message={}",
                package_name,
                include_subpackages,
                message
            );
        }

        last_xml = xml.clone();

        if !looks_like_empty_package_tree(&xml) {
            return Ok(xml);
        }
    }

    Ok(last_xml)
}

fn extract_xml_attr_value(xml: &str, attr_name: &str) -> Option<String> {
    let needle = format!("{}=\"", attr_name);
    let start = xml.find(&needle)? + needle.len();
    let rest = &xml[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn load_package_metadata_summary(
    client: &mut AdtBridgeClient,
    session_id: &str,
    package_name: &str,
) -> Result<(String, String)> {
    let package_uri = format!("/sap/bc/adt/packages/{}", package_name.trim().to_ascii_lowercase());

    let resp = client.send_json(json!({
        "cmd": "call_endpoint",
        "session_id": session_id,
        "method": "GET",
        "uri": package_uri,
        "accept": "application/xml, text/xml, */*"
    }))?;

    let xml = resp
        .get("data")
        .and_then(|v| v.get("body").or_else(|| v.get("xml")))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ADT package metadata response missing body"))?
        .to_string();

    let package_type = extract_xml_attr_value(&xml, "adtcore:type")
        .or_else(|| extract_xml_attr_value(&xml, "type"))
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow!("ADT package metadata did not expose a package type"))?;

    eprintln!(
        "[sap_adt] package metadata package={} uri={} type={}",
        package_name,
        package_uri,
        package_type
    );

    Ok((package_uri, package_type))
}

fn repository_parent_type(object_type: &str) -> &str {
    object_type.split('/').next().unwrap_or(object_type)
}

fn load_nodestructure_xml(
    client: &mut AdtBridgeClient,
    session_id: &str,
    package_name: &str,
    package_uri: &str,
    package_type: &str,
) -> Result<String> {
    let body = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<adtcore:objectReferences xmlns:adtcore=\"http://www.sap.com/adt/core\">",
            "<adtcore:objectReference ",
            "uri=\"{}\" ",
            "name=\"{}\" ",
            "type=\"{}\" ",
            "parentUri=\"{}\" />",
            "</adtcore:objectReferences>"
        ),
        package_uri,
        package_name,
        package_type,
        package_uri
    );

    let nodestructure_uri = format!(
        "/sap/bc/adt/repository/nodestructure?parent_type={}&parent_name={}",
        encode_component(package_type),
        encode_component(package_name)
    );


    let resp = client.send_json(json!({
        "cmd": "call_endpoint",
        "session_id": session_id,
        "method": "POST",
        "uri": nodestructure_uri,
        "accept": "application/vnd.sap.adt.repository.nodestructure.v2+xml, application/xml, */*",
        "content_type": "application/vnd.sap.adt.repository.nodestructure.v2+xml",
        "body": body
    }))?;

    let xml = resp
        .get("data")
        .and_then(|v| v.get("body").or_else(|| v.get("xml")))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("nodestructure response missing body"))?
        .to_string();

    eprintln!(
        "[sap_adt] nodestructure success package={} package_uri={} package_type={} bytes={}",
        package_name, package_uri, package_type, xml.len()
    );

    Ok(xml)
}

fn search_repository_objects_xml(
    client: &mut AdtBridgeClient,
    session_id: &str,
    base_url: &str,
    package_name: &str,
) -> Result<String> {
    let queries = build_package_search_queries(package_name);
    let mut last_xml = String::new();

    for query in queries {
        let mut search_url = Url::parse(&format!("{}/sap/bc/adt/repository/informationsystem/search", base_url))
            .with_context(|| format!("Invalid SAP ADT repository search base URL: {}", base_url))?;
        search_url
            .query_pairs_mut()
            .append_pair("operation", "quickSearch")
            .append_pair("query", &query)
            .append_pair("maxResults", "100");

        let uri = if let Some(q) = search_url.query() {
            format!("{}?{}", search_url.path(), q)
        } else {
            search_url.path().to_string()
        };

        let resp = client.send_json(json!({
            "cmd": "call_endpoint",
            "session_id": session_id,
            "method": "GET",
            "uri": uri,
            "accept": "application/xml, text/xml, */*"
        }))?;

        let xml = resp
            .get("data")
            .and_then(|v| v.get("body").or_else(|| v.get("xml")))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ADT repository search response missing body"))?
            .to_string();

        let preview = compact_xml_preview(&xml, 400);
        eprintln!(
            "[sap_adt] repository search package={} query={} xml_bytes={} xml_preview={}",
            package_name,
            query,
            xml.len(),
            preview
        );

        if let Some(message) = extract_adt_exception_message(&xml) {
            eprintln!(
                "[sap_adt] repository search exception package={} query={} message={}",
                package_name,
                query,
                message
            );
        }

        last_xml = xml.clone();

        if xml.contains("adtcore:objectReference") || xml.contains("objectReference") {
            return Ok(xml);
        }
    }

    if let Some(message) = extract_adt_exception_message(&last_xml) {
        return Err(anyhow!("ADT repository search failed: {}", message));
    }

    Ok(last_xml)
}

fn should_skip_link_target(href: &str, rel: Option<&str>) -> bool {
    let href = href.trim();
    if href.is_empty() {
        return true;
    }
    if href.contains('{') || href.contains('}') {
        return true;
    }
    matches!(rel.unwrap_or_default(), "self" | "supportsPackageCheckActions")
}

fn empty_object_references_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?><adtcore:objectReferences xmlns:adtcore=\"http://www.sap.com/adt/core\"/>".to_string()
}

fn looks_like_global_package_catalog(
    xml: &str,
    package_uri: &str,
    package_type: &str,
) -> bool {
    let parsed = match parse_package_tree_xml(xml) {
        Ok(items) => items,
        Err(_) => return false,
    };

    if parsed.is_empty() {
        return false;
    }

    let all_package_nodes = parsed.iter().all(|item| {
        item.object_type == package_type && item.uri.starts_with("/sap/bc/adt/packages/")
    });

    let has_foreign_package_nodes = parsed.iter().any(|item| {
        item.uri.starts_with("/sap/bc/adt/packages/")
            && !item.uri.eq_ignore_ascii_case(package_uri)
    });

    let has_non_package_nodes = parsed.iter().any(|item| {
        item.object_type != package_type || !item.uri.starts_with("/sap/bc/adt/packages/")
    });

    all_package_nodes && has_foreign_package_nodes && !has_non_package_nodes
}

pub fn list_package_objects(sap: &mut SapAdtState, package_name: &str, include_subpackages: bool) -> Result<String> {
    let package_name = package_name.trim();
    if package_name.is_empty() {
        return Err(anyhow!("Package name is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    eprintln!(
        "[sap_adt] list_package_objects start package={} include_subpackages={} session_id={}",
        package_name,
        include_subpackages,
        session_id
    );

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    if let Ok((package_uri, package_type)) = load_package_metadata_summary(&mut client, &session_id, package_name) {
        match load_nodestructure_xml(&mut client, &session_id, package_name, &package_uri, &package_type) {
            Ok(xml) if xml.trim().is_empty() || looks_like_empty_package_tree(&xml) => {
                eprintln!("[sap_adt] nodestructure returned empty tree for {}", package_name);
                return Ok(empty_object_references_xml());
            }
            Ok(xml) if looks_like_global_package_catalog(&xml, &package_uri, &package_type) => {
                eprintln!(
                    "[sap_adt] nodestructure returned a global package catalog for {} instead of package contents; suppressing package-object fallback",
                    package_name
                );
                return Ok(empty_object_references_xml());
            }
            Ok(xml) => {
                return Ok(xml);
            }
            Err(e) => {
                eprintln!(
                    "[sap_adt] nodestructure failed for package {}: {}; suppressing package-object fallback",
                    package_name,
                    e
                );
                return Ok(empty_object_references_xml());
            }
        }
    }

    eprintln!(
        "[sap_adt] package metadata lookup failed for {}; treating query as a non-package object search",
        package_name
    );
    eprintln!("[sap_adt] falling back to repository search for {}", package_name);
    search_repository_objects_xml(&mut client, &session_id, &base_url, package_name)
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| value.to_string())
}

fn json_headers_to_pairs(value: Option<&serde_json::Value>) -> Vec<(String, String)> {
    value
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), json_value_to_string(v)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn print_terminal_headers(headers: &[(String, String)]) {
    if headers.is_empty() {
        eprintln!("(none)");
        return;
    }
    for (k, v) in headers {
        eprintln!("{}: {}", k, v);
    }
}

fn print_terminal_body(label: &str, body: &str) {
    eprintln!("{}", label);
    if body.is_empty() {
        eprintln!("(empty)");
    } else {
        eprintln!("{}", body);
    }
}

fn dump_adt_trace_to_terminal(trace: &crate::app::state::SapAdtHttpTrace) {
    eprintln!("\n================ SAP ADT HTTP TRACE ================");
    eprintln!("label: {}", trace.label);
    eprintln!("method: {}", trace.method);
    eprintln!("url: {}", trace.url);
    match trace.response_status {
        Some(status) => eprintln!("status: {}", status),
        None => eprintln!("status: (none)"),
    }
    eprintln!("---------------- request headers ----------------");
    print_terminal_headers(&trace.request_headers);
    eprintln!("---------------- request body -------------------");
    print_terminal_body("", &trace.request_body);
    eprintln!("--------------- response headers ----------------");
    print_terminal_headers(&trace.response_headers);
    eprintln!("---------------- response body ------------------");
    print_terminal_body("", &trace.response_body);
    eprintln!("-------------------- error ----------------------");
    match &trace.error {
        Some(error) if !error.is_empty() => eprintln!("{}", error),
        _ => eprintln!("(none)"),
    }
    eprintln!("=================================================\n");
}

fn record_adt_trace(
    sap: &mut SapAdtState,
    label: &str,
    method: &str,
    url: &str,
    request_headers: Vec<(String, String)>,
    request_body: String,
    response_status: Option<u16>,
    response_headers: Vec<(String, String)>,
    response_body: String,
    error: Option<String>,
) {
    let trace = crate::app::state::SapAdtHttpTrace {
        label: label.to_string(),
        method: method.to_string(),
        url: url.to_string(),
        request_headers,
        request_body,
        response_status,
        response_headers,
        response_body,
        error,
    };

    dump_adt_trace_to_terminal(&trace);

    if sap.debug_http_enabled {
        sap.last_http_trace = Some(trace);
    }
}

pub fn debug_accept_matrix(
    sap: &mut SapAdtState,
    object_uri: &str,
) -> Result<Vec<crate::app::state::SapAdtAcceptProbe>> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let accepts = [
        "application/vnd.sap.adt.basic.object.properties+xml",
        "application/vnd.sap.adt.basic.object.properties+xml, application/xml, text/xml, */*",
        "application/xml, text/xml, */*",
        "text/plain, text/*",
        "*/*",
    ];

    let mut results = Vec::new();

    for accept in accepts {
        let request_headers = vec![("accept".to_string(), accept.to_string())];
        let resp = client.send_json(json!({
            "cmd": "read_object",
            "session_id": session_id,
            "object_uri": object_uri,
            "accept": accept
        }));

        match resp {
            Ok(resp) => {
                let data = resp.get("data").unwrap_or(&resp);
                let status = data
                    .get("status")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u16)
                    .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));
                let headers = json_headers_to_pairs(data.get("headers").or_else(|| resp.get("headers")));
                let body = data
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let content_type = data
                    .get("content_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        headers.iter().find_map(|(k, v)| {
                            if k.eq_ignore_ascii_case("content-type") {
                                Some(v.clone())
                            } else {
                                None
                            }
                        })
                    });
                let response_preview = body.chars().take(400).collect::<String>();

                record_adt_trace(
                    sap,
                    "debug_accept_matrix",
                    "GET",
                    object_uri,
                    request_headers,
                    String::new(),
                    status,
                    headers.clone(),
                    body.clone(),
                    None,
                );

                results.push(crate::app::state::SapAdtAcceptProbe {
                    accept: accept.to_string(),
                    status,
                    content_type,
                    response_preview,
                    error: None,
                });
            }
            Err(err) => {
                let error_text = format!("{:#}", err);
                let status = regex::Regex::new(r"\(([0-9]{3})\)")
                    .ok()
                    .and_then(|re| re.captures(&error_text))
                    .and_then(|caps| caps.get(1))
                    .and_then(|m| m.as_str().parse::<u16>().ok());

                record_adt_trace(
                    sap,
                    "debug_accept_matrix",
                    "GET",
                    object_uri,
                    request_headers,
                    String::new(),
                    status,
                    Vec::new(),
                    String::new(),
                    Some(error_text.clone()),
                );

                results.push(crate::app::state::SapAdtAcceptProbe {
                    accept: accept.to_string(),
                    status,
                    content_type: None,
                    response_preview: String::new(),
                    error: Some(error_text),
                });
            }
        }
    }

    Ok(results)
}

pub fn read_object(sap: &mut SapAdtState, object_uri: &str, accept: Option<&str>) -> Result<AdtReadObjectResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let accept_value = accept.unwrap_or("application/vnd.sap.adt.basic.object.properties+xml, text/plain, text/*, application/xml, text/xml, */*");
    let request_headers = vec![("accept".to_string(), accept_value.to_string())];
    let resp = client.send_json(json!({
        "cmd": "read_object",
        "session_id": session_id,
        "object_uri": object_uri,
        "accept": accept_value
    }));

    match resp {
        Ok(resp) => {
            let data = resp.get("data").ok_or_else(|| anyhow!("ADT read_object response missing data"))?;
            let body = data
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("ADT read_object response missing body"))?
                .to_string();
            let headers = json_headers_to_pairs(data.get("headers"));
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            record_adt_trace(
                sap,
                "read_object",
                "GET",
                object_uri,
                request_headers,
                String::new(),
                status,
                headers.clone(),
                body.clone(),
                None,
            );

            Ok(AdtReadObjectResult {
                object_uri: object_uri.to_string(),
                content_type: data.get("content_type").and_then(|v| v.as_str()).map(|s| s.to_string()),
                headers,
                body,
            })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = regex::Regex::new(r"\(([0-9]{3})\)")
                .ok()
                .and_then(|re| re.captures(&error_text))
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse::<u16>().ok());

            record_adt_trace(
                sap,
                "read_object",
                "GET",
                object_uri,
                request_headers,
                String::new(),
                status,
                Vec::new(),
                String::new(),
                Some(error_text.clone()),
            );

            Err(anyhow!(error_text))
        }
    }
}

pub fn lock_object(sap: &mut SapAdtState, object_uri: &str) -> Result<AdtLockObjectResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "lock_object",
        "session_id": session_id,
        "object_uri": object_uri
    }));

    match resp {
        Ok(resp) => {
            let data = resp.get("data").ok_or_else(|| anyhow!("ADT lock_object response missing data"))?;
            let headers = json_headers_to_pairs(data.get("headers"));
            let body = data
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let lock_handle = data
                .get("lock_handle")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("ADT lock_object response missing lock_handle"))?
                .to_string();
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            record_adt_trace(
                sap,
                "lock_object",
                "POST",
                object_uri,
                Vec::new(),
                String::new(),
                status,
                headers.clone(),
                body.clone(),
                None,
            );

            Ok(AdtLockObjectResult {
                object_uri: object_uri.to_string(),
                lock_handle,
                headers,
                body,
            })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = regex::Regex::new(r"\(([0-9]{3})\)")
                .ok()
                .and_then(|re| re.captures(&error_text))
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse::<u16>().ok());

            record_adt_trace(
                sap,
                "lock_object",
                "POST",
                object_uri,
                Vec::new(),
                String::new(),
                status,
                Vec::new(),
                String::new(),
                Some(error_text.clone()),
            );

            Err(anyhow!(error_text))
        }
    }
}

pub fn update_object(
    sap: &mut SapAdtState,
    object_uri: &str,
    body: &str,
    content_type: Option<&str>,
    lock_handle: Option<&str>,
    corr_nr: Option<&str>,
    if_match: Option<&str>,
) -> Result<AdtUpdateObjectResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let mut headers = serde_json::Map::new();
    if let Some(if_match) = if_match.filter(|s| !s.trim().is_empty()) {
        headers.insert("If-Match".to_string(), serde_json::Value::String(if_match.to_string()));
    }

    let content_type_value = content_type.unwrap_or("text/plain; charset=utf-8").to_string();
    let mut request_headers = vec![("content-type".to_string(), content_type_value.clone())];
    if let Some(if_match) = if_match.filter(|s| !s.trim().is_empty()) {
        request_headers.push(("if-match".to_string(), if_match.to_string()));
    }
    if let Some(lock_handle) = lock_handle.filter(|s| !s.trim().is_empty()) {
        request_headers.push(("x-sap-adt-lockhandle".to_string(), lock_handle.to_string()));
    }

    let resp = client.send_json(json!({
        "cmd": "update_object",
        "session_id": session_id,
        "object_uri": object_uri,
        "source": body,
        "content_type": content_type_value,
        "lock_handle": lock_handle,
        "corr_nr": corr_nr,
        "headers": headers
    }));

    match resp {
        Ok(resp) => {
            let data = resp.get("data").ok_or_else(|| anyhow!("ADT update_object response missing data"))?;
            let headers = json_headers_to_pairs(data.get("headers"));
            let body_text = data
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            record_adt_trace(
                sap,
                "update_object",
                "PUT",
                object_uri,
                request_headers,
                body.to_string(),
                status,
                headers.clone(),
                body_text.clone(),
                None,
            );

            Ok(AdtUpdateObjectResult { headers, body: body_text })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = regex::Regex::new(r"\(([0-9]{3})\)")
                .ok()
                .and_then(|re| re.captures(&error_text))
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse::<u16>().ok());

            record_adt_trace(
                sap,
                "update_object",
                "PUT",
                object_uri,
                request_headers,
                body.to_string(),
                status,
                Vec::new(),
                String::new(),
                Some(error_text.clone()),
            );

            Err(anyhow!(error_text))
        }
    }
}

pub fn syntax_check(sap: &mut SapAdtState, object_uri: &str) -> Result<AdtCheckResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "syntax_check",
        "session_id": session_id,
        "object_uri": object_uri
    }));

    match resp {
        Ok(resp) => {
            let data = resp.get("data").unwrap_or(&resp);
            let body = data
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let headers = json_headers_to_pairs(data.get("headers").or_else(|| resp.get("headers")));
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            record_adt_trace(
                sap,
                "syntax_check",
                "POST",
                object_uri,
                Vec::new(),
                String::new(),
                status,
                headers,
                body.clone(),
                None,
            );

            Ok(AdtCheckResult { body })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = regex::Regex::new(r"\(([0-9]{3})\)")
                .ok()
                .and_then(|re| re.captures(&error_text))
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse::<u16>().ok());

            record_adt_trace(
                sap,
                "syntax_check",
                "POST",
                object_uri,
                Vec::new(),
                String::new(),
                status,
                Vec::new(),
                String::new(),
                Some(error_text.clone()),
            );

            Err(anyhow!(error_text))
        }
    }
}

pub fn activate_object(sap: &mut SapAdtState, object_uri: &str) -> Result<AdtActivateResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "activate_object",
        "session_id": session_id,
        "object_uri": object_uri
    }));

    match resp {
        Ok(resp) => {
            let data = resp.get("data").unwrap_or(&resp);
            let body = data
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let headers = json_headers_to_pairs(data.get("headers").or_else(|| resp.get("headers")));
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            record_adt_trace(
                sap,
                "activate_object",
                "POST",
                object_uri,
                Vec::new(),
                String::new(),
                status,
                headers,
                body.clone(),
                None,
            );

            Ok(AdtActivateResult { body })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = regex::Regex::new(r"\(([0-9]{3})\)")
                .ok()
                .and_then(|re| re.captures(&error_text))
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse::<u16>().ok());

            record_adt_trace(
                sap,
                "activate_object",
                "POST",
                object_uri,
                Vec::new(),
                String::new(),
                status,
                Vec::new(),
                String::new(),
                Some(error_text.clone()),
            );

            Err(anyhow!(error_text))
        }
    }
}
