use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::Url;
use serde_json::{json, Value};

use crate::engine::capabilities::inference::BrowserConfig as BrowserTurnConfig;
use crate::engine::capabilities::inference::browser::adapter as browser_bridge;
use crate::engine::capabilities::sap::adt::client::AdtClient;
use super::sap_adt_manifest::{
    SapAdtManifestDocument,
    SapAdtManifestResource,
    SapAdtObjectManifest,
};
use crate::engine::capabilities::sap::state::{
    SapAdtObjectSummary,
    SapAdtState,
};

#[derive(Clone, Debug)]
pub struct AdtReadObjectResult {
    pub object_uri: String,
    pub content_type: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtUpdateObjectResult {
    pub status: Option<u16>,
    pub body: String,
    pub problems: Vec<String>,
    pub ok: bool,
}

#[derive(Clone, Debug)]
pub struct AdtCheckResult {
    pub status: Option<u16>,
    pub body: String,
    pub problems: Vec<String>,
    pub ok: bool,
}

#[derive(Clone, Debug)]
pub struct AdtActivateResult {
    pub status: Option<u16>,
    pub body: String,
    pub problems: Vec<String>,
    pub ok: bool,
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
                if !href.trim().is_empty()
                    && (content_type.as_deref() == Some("text/plain") || content_type.is_none())
                {
                    return Ok(Some(resolve_relative_object_uri(metadata_uri, &href)?));
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

fn metadata_package_ref_name(metadata_xml: &str) -> Option<String> {
    let re = Regex::new(r#"<(?:(?:[^\s>]+):)?packageRef\b[^>]*\badtcore:name=\"([^\"]+)\"[^>]*/?>"#).ok()?;
    let caps = re.captures(metadata_xml)?;
    let value = caps.get(1)?.as_str().trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
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

    if (rel_lc.contains("source") || title_lc.contains("source") || uri_lc.contains("source"))
        && (title_lc.contains("unicode")
            || uri_lc.contains("source_standard_abap_unicode")
            || uri_lc.contains("standard_abap_unicode")
            || rel_lc.contains("standardabapunicode")
            || ct_lc.contains("abap") && body.len() > 200000)
    {
        return true;
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

    log_manifest_crawl_step(&format!(
        "metadata full response uri={} fallback_object_name={} fallback_object_type={} fallback_package_name={} xml={} ",
        metadata_uri,
        object_name.unwrap_or(""),
        object_type.unwrap_or(""),
        package_name.unwrap_or(""),
        metadata_xml
    ));

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
        headers: Vec::new(),
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
                    etag: None,
                    lock_handle: None,
                    headers: Vec::new(),
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
                    headers: Vec::new(),
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
            if seen_uris.insert(resolved_uri.clone())
                && !should_skip_manifest_artifact(
                    "http://www.sap.com/adt/relations/source",
                    Some("source"),
                    read_result.content_type.as_deref(),
                    &resolved_uri,
                    &read_result.body,
                )
            {
                resources.push(SapAdtManifestResource {
                    id: "resource_1".to_string(),
                    uri: resolved_uri.clone(),
                    rel: "http://www.sap.com/adt/relations/source".to_string(),
                    title: Some("source".to_string()),
                    content_type: read_result.content_type.clone(),
                    etag: None,
                    lock_handle: None,
                    headers: Vec::new(),
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
    )
    .or_else(|| metadata_package_ref_name(&metadata_xml));
    let object_uri = extract_object_source_uri(metadata_uri, &metadata_xml)?
        .and_then(|source_uri| resolve_relative_object_uri_candidates(metadata_uri, &source_uri).ok())
        .and_then(|candidates| candidates.into_iter().find(|candidate| seen_uris.contains(candidate)))
        .or_else(|| resources.first().map(|resource| resource.uri.clone()));
    let manifest = SapAdtObjectManifest {
        schema_version: 1,
        metadata_uri: metadata_uri.to_string(),
        object_uri,
        object_name: object_name.map(|v| v.to_string()).or(root_object_name),
        object_type: object_type.map(|v| v.to_string()).or(root_object_type),
        package_name: package_name.map(|v| v.to_string()).or(root_package_name),
        etag: None,
        metadata_xml,
        resources,
        documents,
    };
    log_manifest_summary(&manifest);
    Ok(manifest)
}

struct SharedAdtSession {
    cache_key: String,
    session_id: String,
}

struct AdtBridgeClient {
    session: Option<SharedAdtSession>,
    pending_state: Option<SapAdtState>,
    next_id: u64,
}

impl AdtBridgeClient {
    fn new() -> Self {
        Self {
            session: None,
            pending_state: None,
            next_id: 1,
        }
    }

    fn set_state(&mut self, sap: &SapAdtState) {
        self.pending_state = Some(sap.clone());
    }

    fn ensure_started(&mut self, _bridge_dir: &str, _base_url: &str) -> Result<()> {
        let state = self
            .pending_state
            .clone()
            .ok_or_else(|| anyhow!("ADT native client state unavailable"))?;
        let cache_key = native_session_cache_key(&state);

        let reuse_existing = self
            .session
            .as_ref()
            .map(|session| session.cache_key == cache_key)
            .unwrap_or(false);

        if reuse_existing {
            return Ok(());
        }

        let session_id = format!("adt-native-{}", self.command_id());
        self.session = Some(SharedAdtSession {
            cache_key,
            session_id,
        });
        Ok(())
    }

    fn command_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        format!("adt-{}", id)
    }

    fn session_ref(&self) -> Result<&SharedAdtSession> {
        self.session
            .as_ref()
            .ok_or_else(|| anyhow!("ADT native client session not connected"))
    }

    fn run_with_native_client<T, F>(&self, op: F) -> Result<T>
    where
        F: FnOnce(&mut AdtClient) -> Result<T>,
    {
        let state = self
            .pending_state
            .clone()
            .ok_or_else(|| anyhow!("ADT native client state unavailable"))?;

        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| {
                let mut client = AdtClient::new(&state)?;
                op(&mut client)
            })
        } else {
            let mut client = AdtClient::new(&state)?;
            op(&mut client)
        }
    }

    fn send_json(&mut self, mut payload: Value) -> Result<Value> {
        let id = self.command_id();
        payload["id"] = Value::String(id.clone());

        let cmd = payload
            .get("cmd")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ADT command missing cmd"))?
            .to_string();

        if cmd == "connect" {
            if let Some(cookie_header) = payload
                .get("cookie_header")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                if let Some(state) = self.pending_state.as_mut() {
                    state.cookie_header = Some(cookie_header.to_string());
                }
            }
            self.ensure_started("", "")?;
            let session = self.session_ref()?;
            return Ok(json!({
                "id": id,
                "ok": true,
                "cmd": cmd,
                "session_id": session.session_id,
                "data": {
                    "session_id": session.session_id,
                    "connected": true,
                    "cookie_header": self.pending_state.as_ref().and_then(|state| state.cookie_header.clone())
                }
            }));
        }

        let session_id = self.session_ref()?.session_id.clone();
        let data = match cmd.as_str() {
            "call_endpoint" => {
                let method = payload.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_string();
                let uri = payload
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("call_endpoint missing uri"))?
                    .to_string();
                let body = payload.get("body").and_then(|v| v.as_str()).map(|v| v.to_string());
                let content_type = payload.get("content_type").and_then(|v| v.as_str()).map(|v| v.to_string());
                let accept = payload.get("accept").and_then(|v| v.as_str()).map(|v| v.to_string());
                let headers = json_headers_to_pairs(payload.get("headers"));
                self.run_with_native_client(|client| {
                    let resp = client.call_endpoint(
                        &method,
                        &uri,
                        body.as_deref(),
                        content_type.as_deref(),
                        accept.as_deref(),
                        Some(headers),
                    )?;
                    if !(200..300).contains(&resp.status) {
                        let message = if resp.body.trim().is_empty() {
                            format!("ADT call_endpoint failed ({}) {} {}", resp.status, method, uri)
                        } else {
                            format!("ADT call_endpoint failed ({}) {} {}: {}", resp.status, method, uri, resp.body)
                        };
                        return Err(anyhow!(message));
                    }
                    Ok(json!({
                        "status": resp.status,
                        "headers": resp.headers,
                        "body": resp.body,
                        "xml": resp.body
                    }))
                })?
            }
            "read_object" => {
                let object_uri = payload
                    .get("object_uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("read_object missing object_uri"))?
                    .to_string();
                let accept = payload.get("accept").and_then(|v| v.as_str()).map(|v| v.to_string());
                self.run_with_native_client(|client| {
                    let resp = client.call_endpoint(
                        "GET",
                        &object_uri,
                        None,
                        None,
                        accept.as_deref(),
                        None,
                    )?;
                    if !(200..300).contains(&resp.status) {
                        let message = if resp.body.trim().is_empty() {
                            format!("ADT read_object failed ({}) {}", resp.status, object_uri)
                        } else {
                            format!("ADT read_object failed ({}) {}: {}", resp.status, object_uri, resp.body)
                        };
                        return Err(anyhow!(message));
                    }
                    Ok(json!({
                        "object_uri": object_uri,
                        "content_type": header_value(&resp.headers, "content-type"),
                        "body": resp.body,
                        "status": resp.status,
                        "headers": resp.headers
                    }))
                })?
            }
            "lock_object" => {
                let object_uri = payload
                    .get("object_uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("lock_object missing object_uri"))?
                    .to_string();
                self.run_with_native_client(|client| {
                    let lock_resp = if object_uri.contains("/ddic/ddl/sources/") {
                        client.lock_ddl_source(&object_uri)?
                    } else {
                        client.lock_object(&object_uri)?
                    };

                    let lock_handle = extract_tag_value(&lock_resp.body, "LOCK_HANDLE")
                        .or_else(|| extract_tag_value(&lock_resp.body, "lockHandle"))
                        .ok_or_else(|| anyhow!(format!("lock_object did not return LOCK_HANDLE for {}", object_uri)))?;

                    Ok(json!({
                        "status": 200,
                        "headers": [],
                        "body": lock_resp.body,
                        "lock_handle": lock_handle
                    }))
                })?
            }
            "update_object" => {
                let object_uri = payload
                    .get("object_uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("update_object missing object_uri"))?
                    .to_string();
                let source = payload.get("source").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                let content_type = payload.get("content_type").and_then(|v| v.as_str()).map(|v| v.to_string());
                let lock_handle = payload.get("lock_handle").and_then(|v| v.as_str()).map(|v| v.to_string());
                let corr_nr = payload.get("corr_nr").and_then(|v| v.as_str()).map(|v| v.to_string());
                let headers = json_headers_to_pairs(payload.get("headers"));
                self.run_with_native_client(|client| {
                    let route = resolve_adt_route(&object_uri);
                    let is_ddl_source = matches!(route.route_family, AdtResolvedRouteFamily::RootToSourceMain)
                        && route.lock_uri.contains("/ddic/ddl/sources/");
                    let mut derived_lock_handle = lock_handle.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
                    let mut derived_corr_nr = corr_nr.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

                    if derived_lock_handle.is_none() {
                        let lock_resp = if is_ddl_source {
                            client.lock_ddl_source(&route.lock_uri)
                        } else {
                            client.lock_object(&route.lock_uri)
                        };

                        let lock_resp = match lock_resp {
                            Ok(resp) => resp,
                            Err(err) => {
                                let error_text = format!("{:#}", err);
                                let body = extract_xml_payload_from_error_text(&error_text);
                                let message = extract_adt_exception_message(&body)
                                    .or_else(|| extract_adt_exception_message(&error_text))
                                    .unwrap_or_else(|| error_text.clone());
                                return Err(anyhow!(format!(
                                    "update_object lock failed for {}: {}",
                                    route.lock_uri,
                                    message
                                )));
                            }
                        };

                        derived_lock_handle = extract_tag_value(&lock_resp.body, "LOCK_HANDLE")
                            .or_else(|| extract_tag_value(&lock_resp.body, "lockHandle"));
                        derived_corr_nr = extract_tag_value(&lock_resp.body, "CORRNR")
                            .or_else(|| extract_tag_value(&lock_resp.body, "corrNr"))
                            .or(derived_corr_nr);
                        if derived_lock_handle.is_none() {
                            let body_message = extract_adt_exception_message(&lock_resp.body)
                                .unwrap_or_else(|| lock_resp.body.chars().take(500).collect::<String>());
                            return Err(anyhow!(format!(
                                "update_object lock failed for {}: {}",
                                route.lock_uri,
                                body_message
                            )));
                        }
                    }

                    let mut final_uri = route.write_uri.clone();
                    let mut query_parts = Vec::new();
                    if let Some(lock_handle) = derived_lock_handle.as_deref() {
                        query_parts.push(format!("lockHandle={}", urlencoding::encode(lock_handle)));
                    }
                    if let Some(corr_nr) = derived_corr_nr.as_deref() {
                        query_parts.push(format!("corrNr={}", urlencoding::encode(corr_nr)));
                    }
                    if !query_parts.is_empty() {
                        final_uri.push(if final_uri.contains('?') { '&' } else { '?' });
                        final_uri.push_str(&query_parts.join("&"));
                    }

                    let source_to_write = if is_ddl_source {
                        let fmt_resp = client.format_ddl_identifiers(&source)?;
                        if !fmt_resp.body.trim().is_empty() {
                            fmt_resp.body
                        } else {
                            source.clone()
                        }
                    } else {
                        source.clone()
                    };

                    let result = client.call_endpoint(
                        "PUT",
                        &final_uri,
                        Some(&source_to_write),
                        content_type.as_deref(),
                        None,
                        Some(headers),
                    );

                    if let Some(lock_handle) = derived_lock_handle.as_deref() {
                        let _ = client.unlock_object(&route.lock_uri, lock_handle);
                    }

                    let resp = result?;
                    let ok = (200..300).contains(&resp.status);
                    Ok(json!({
                        "status": resp.status,
                        "body": resp.body,
                        "problems": [],
                        "ok": ok,
                        "headers": {
                            "x-adt-lock-uri": route.lock_uri,
                            "x-adt-write-uri": route.write_uri
                        }
                    }))
                })?
            }
            "syntax_check" => {
                let object_uri = payload
                    .get("object_uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("syntax_check missing object_uri"))?
                    .to_string();
                self.run_with_native_client(|client| {
                    let body = format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><chkrun:checkObjectList xmlns:chkrun=\"http://www.sap.com/adt/checkrun\" xmlns:adtcore=\"http://www.sap.com/adt/core\"><chkrun:checkObject adtcore:uri=\"{}\" chkrun:version=\"inactive\"/></chkrun:checkObjectList>",
                        object_uri
                            .replace('&', "&amp;")
                            .replace('"', "&quot;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                    );
                    let resp = client.call_endpoint(
                        "POST",
                        "/sap/bc/adt/checkruns?reporters=abapCheckRun",
                        Some(&body),
                        Some("application/vnd.sap.adt.checkobjects+xml"),
                        Some("application/vnd.sap.adt.checkmessages+xml"),
                        None,
                    )?;
                    Ok(json!({
                        "status": resp.status,
                        "body": resp.body
                    }))
                })?
            }
            "activate_object" => {
                let object_uri = payload
                    .get("object_uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("activate_object missing object_uri"))?
                    .to_string();
                self.run_with_native_client(|client| {
                    let body = format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><adtcore:objectReferences xmlns:adtcore=\"http://www.sap.com/adt/core\"><adtcore:objectReference adtcore:uri=\"{}\" /></adtcore:objectReferences>",
                        object_uri
                            .replace('&', "&amp;")
                            .replace('"', "&quot;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                    );
                    let resp = client.call_endpoint(
                        "POST",
                        "/sap/bc/adt/activation?method=activate&preauditRequested=true",
                        Some(&body),
                        Some("application/xml, text/xml, */*"),
                        None,
                        Some(vec![("Content-Type".to_string(), "application/xml; charset=utf-8".to_string())]),
                    )?;
                    Ok(json!({
                        "status": resp.status,
                        "body": resp.body
                    }))
                })?
            }
            other => {
                return Err(anyhow!(format!("Unhandled ADT native command {}", other)));
            }
        };

        Ok(json!({
            "id": id,
            "ok": true,
            "cmd": cmd,
            "session_id": session_id,
            "data": data
        }))
    }
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn native_session_cache_key(sap: &SapAdtState) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        sap.base_url.trim().trim_end_matches('/').to_ascii_lowercase(),
        sap.client.trim().to_ascii_lowercase(),
        sap.auth_type.trim().to_ascii_lowercase(),
        sap.authorization.trim(),
        sap.cookie_header.as_deref().unwrap_or("").trim()
    )
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

    let configured = sap.bridge_dir.trim();
    if !configured.is_empty() {
        return configured.to_string();
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

fn browser_executable_for_adt() -> String {
    String::new()
}

fn browser_page_match_url(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        return format!(
            "{}://{}{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or_default(),
            parsed.port().map(|p| format!(":{}", p)).unwrap_or_default()
        );
    }

    url.trim().to_string()
}

fn browser_cfg_from_state(sap: &SapAdtState) -> BrowserTurnConfig {
    let discovery_url = sap
        .discovery_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("https://website.com/")
        .to_string();
    let target_url = explicit_flp_url(&discovery_url).unwrap_or_else(|| discovery_url.clone());
    let page_url_contains = browser_page_match_url(&target_url);

    BrowserTurnConfig {
        edge_executable: browser_executable_for_adt(),
        user_data_dir: String::new(),
        cdp_url: "http://127.0.0.1:9222".to_string(),
        page_url_contains,
        profile: browser_profile_for_adt(sap),
        session_id: sap.browser_session_id.clone(),
        auto_launch_edge: true,
        target_url,
        response_timeout_ms: 45_000,
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

fn validate_bridge_settings(_sap: &SapAdtState) -> Result<()> {
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

fn explicit_flp_url(discovery_url: &str) -> Option<String> {
    let parsed = Url::parse(discovery_url).ok()?;
    let mut flp = parsed;
    flp.set_path("/sap/bc/ui5_ui5/ui2/ushell/shells/abap/FioriLaunchpad.html");
    flp.set_fragment(Some("Shell-home"));

    let mut sap_client = None::<String>;
    let mut sap_language = None::<String>;
    for (key, value) in flp.query_pairs() {
        if key.eq_ignore_ascii_case("sap-client") {
            sap_client = Some(value.into_owned());
        } else if key.eq_ignore_ascii_case("sap-language") {
            sap_language = Some(value.into_owned());
        }
    }

    let sap_client = sap_client.unwrap_or_else(|| "010".to_string());
    flp.query_pairs_mut().clear().append_pair("sap-client", &sap_client);
    if let Some(language) = sap_language {
        flp.query_pairs_mut().append_pair("sap-language", &language);
    }

    Some(flp.to_string())
}

fn cookie_harvest_navigation_urls(discovery_url: &str) -> Vec<String> {
    let mut urls = Vec::new();

    if let Some(flp_url) = explicit_flp_url(discovery_url) {
        if !urls.iter().any(|u| u == &flp_url) {
            urls.push(flp_url);
        }
    }

    let trimmed = discovery_url.trim();
    if !trimmed.is_empty() {
        let raw = trimmed.to_string();
        if !urls.iter().any(|u| u == &raw) {
            urls.push(raw);
        }
    }

    urls
}

fn cookie_harvest_lookup_urls(discovery_url: &str) -> Vec<String> {
    let mut urls = cookie_urls_for_discovery(discovery_url);

    if let Some(flp_url) = explicit_flp_url(discovery_url) {
        if !urls.iter().any(|u| u == &flp_url) {
            urls.push(flp_url.clone());
        }
        if let Ok(url) = Url::parse(&flp_url) {
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
    }

    urls
}

fn has_cookie_harvest_signal(cookie_header: &str) -> bool {
    let names: Vec<String> = cookie_header
        .split(';')
        .filter_map(|part| part.split('=').next())
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();

    let has_mysapsso2 = names.iter().any(|name| name == "mysapsso2");
    let has_session = names.iter().any(|name| name.starts_with("sap_sessionid"));
    let has_usercontext = names.iter().any(|name| name == "sap-usercontext");

    !cookie_header.trim().is_empty() && ((has_mysapsso2 && has_session) || has_usercontext)
}

fn reset_browser_session(cfg: &mut BrowserTurnConfig) {
    cfg.session_id = None;
}

fn attach_browser_session(cfg: &mut BrowserTurnConfig) -> Result<()> {
    browser_bridge::launch_and_attach(cfg)?;
    Ok(())
}

const MAX_COOKIE_HARVEST_OPEN_ATTEMPTS: usize = 3;

fn wait_for_stable_browser_url(cfg: &mut BrowserTurnConfig, expected_prefix: &str) -> Result<String> {
    let started = std::time::Instant::now();
    let timeout = Duration::from_secs(90);
    let stable_for = Duration::from_secs(3);
    let poll = Duration::from_millis(500);
    let expected_prefix = expected_prefix.trim().to_ascii_lowercase();
    let mut last_url = String::new();
    let mut last_seen = String::new();
    let mut stable_since: Option<std::time::Instant> = None;
    let mut saw_expected_host = expected_prefix.is_empty();

    loop {
        if started.elapsed() >= timeout {
            return Err(anyhow!(format!("Timed out waiting for browser URL to stabilize after SSO redirects (last_url={})", last_seen)));
        }

        let probe = browser_bridge::probe(cfg)?;
        let current_url = probe.url.trim().to_string();
        if current_url.is_empty() {
            stable_since = None;
            std::thread::sleep(poll);
            continue;
        }

        last_seen = current_url.clone();
        let matches_expected = expected_prefix.is_empty() || current_url.to_ascii_lowercase().starts_with(&expected_prefix);
        if matches_expected {
            saw_expected_host = true;
        }

        if !saw_expected_host {
            last_url = current_url;
            stable_since = None;
            std::thread::sleep(poll);
            continue;
        }

        if current_url == last_url {
            if let Some(since) = stable_since {
                if since.elapsed() >= stable_for && probe.page_open {
                    return Ok(current_url);
                }
            } else {
                stable_since = Some(std::time::Instant::now());
            }
        } else {
            last_url = current_url;
            stable_since = Some(std::time::Instant::now());
        }

        std::thread::sleep(poll);
    }
}

fn cached_cookie_header(sap: &SapAdtState) -> Option<String> {
    sap.cookie_header
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| has_cookie_harvest_signal(value))
}

pub(crate) fn refresh_cookie_header(sap: &mut SapAdtState, discovery_url: &str) -> Result<String> {
    validate_bridge_settings(sap)?;

    let navigation_urls = cookie_harvest_navigation_urls(discovery_url);
    let lookup_urls = cookie_harvest_lookup_urls(discovery_url);
    if let Some(cookie_header) = cached_cookie_header(sap) {
        sap.browser_session_id = None;
        return Ok(cookie_header);
    }

    let mut last_err = None::<anyhow::Error>;
    let mut last_cookie_header = String::new();
    let mut open_attempts = 0usize;

    for (index, url) in navigation_urls.iter().enumerate() {
        let mut cfg = browser_cfg_from_state(sap);
        let expected_prefix = cfg.page_url_contains.clone();
        let had_existing_session = cfg
            .session_id
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);

        if !had_existing_session {
            reset_browser_session(&mut cfg);
        }

        if let Err(err) = attach_browser_session(&mut cfg) {
            last_err = Some(err.context(format!("Failed to attach browser bridge session for SAP ADT target {}: {}", index + 1, url)));
            continue;
        }

        if had_existing_session {
            match browser_bridge::get_session_cookies(&mut cfg, &lookup_urls) {
                Ok(cookie_header) => {
                    if has_cookie_harvest_signal(&cookie_header) {
                        sap.browser_session_id = None;
                        sap.cookie_header = Some(cookie_header.clone());
                        return Ok(cookie_header);
                    }
                    last_cookie_header = cookie_header;
                }
                Err(err) => {
                    last_err = Some(err.context("Failed to harvest SAP ADT cookies from existing browser session"));
                }
            }
        }

        if open_attempts >= MAX_COOKIE_HARVEST_OPEN_ATTEMPTS {
            last_err = Some(anyhow!(format!(
                "Exceeded maximum SAP browser open attempts ({}) before harvesting cookies",
                MAX_COOKIE_HARVEST_OPEN_ATTEMPTS
            )));
            break;
        }

        open_attempts += 1;
        if let Err(err) = browser_bridge::open_url(&mut cfg, url) {
            last_err = Some(err.context(format!(
                "Failed to open SAP ADT browser URL attempt {} of {}: {}",
                open_attempts,
                MAX_COOKIE_HARVEST_OPEN_ATTEMPTS,
                url
            )));
            continue;
        }

        if let Err(err) = wait_for_stable_browser_url(&mut cfg, &expected_prefix) {
            last_err = Some(err.context(format!(
                "Failed waiting for stable SAP browser URL after open attempt {} of {}: {}",
                open_attempts,
                MAX_COOKIE_HARVEST_OPEN_ATTEMPTS,
                url
            )));
            continue;
        }

        match browser_bridge::get_session_cookies(&mut cfg, &lookup_urls) {
            Ok(cookie_header) => {
                if has_cookie_harvest_signal(&cookie_header) {
                    sap.browser_session_id = None;
                    sap.cookie_header = Some(cookie_header.clone());
                    return Ok(cookie_header);
                }
                last_cookie_header = cookie_header;
            }
            Err(err) => {
                last_err = Some(err.context(format!(
                    "Failed to harvest SAP ADT cookies after open attempt {} of {}: {}",
                    open_attempts,
                    MAX_COOKIE_HARVEST_OPEN_ATTEMPTS,
                    url
                )));
            }
        }
    }

    sap.browser_session_id = None;
    if !last_cookie_header.trim().is_empty() {
        sap.cookie_header = Some(last_cookie_header.clone());
        return Ok(last_cookie_header);
    }

    if let Some(err) = last_err {
        return Err(err);
    }

    Err(anyhow!("Failed to harvest SAP ADT cookies from browser bridge after all URL attempts"))
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
    client.set_state(sap);
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

    let discovery_url = require_discovery_url(sap)?;

    let mut cookie_header = sap
        .cookie_header
        .clone()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_default();

    if cookie_header.trim().is_empty() {
        cookie_header = refresh_cookie_header(sap, &discovery_url)?;
    }

    match connect_transport_session(sap, &cookie_header) {
        Ok(session_id) => Ok(session_id),
        Err(err) => {
            let message = err.to_string().to_ascii_lowercase();
            let should_retry = message.contains("401")
                || message.contains("403")
                || message.contains("unauthorized")
                || message.contains("forbidden")
                || message.contains("cookie")
                || message.contains("csrf")
                || message.contains("session");

            if !should_retry {
                return Err(err);
            }

            let refreshed_cookie_header = refresh_cookie_header(sap, &discovery_url)?;
            connect_transport_session(sap, &refreshed_cookie_header)
        }
    }
}

fn xml_tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
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

    Ok(out)
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

fn looks_like_empty_package_tree(xml: &str) -> bool {
    let trimmed = xml.trim();
    let lower = trimmed.to_ascii_lowercase();
    let has_package_tree = lower.contains("<packagetree") || lower.contains(":packagetree");
    let has_object_reference = lower.contains("<objectreference") || lower.contains(":objectreference");
    let has_uri_attr = lower.contains(" uri=") || lower.contains(" adtcore:uri=") || lower.contains("\nuri=") || lower.contains("\nadtcore:uri=");
    has_package_tree && !has_object_reference && !has_uri_attr
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

    Ok((package_uri, package_type))
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

pub fn list_package_objects(sap: &mut SapAdtState, package_name: &str, _include_subpackages: bool) -> Result<String> {
    let package_name = package_name.trim();
    if package_name.is_empty() {
        return Err(anyhow!("Package name is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    if let Ok((package_uri, package_type)) = load_package_metadata_summary(&mut client, &session_id, package_name) {
    client.set_state(sap);
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

fn log_adt_http(
    label: &str,
    method: &str,
    url: &str,
    response_status: Option<u16>,
    error: Option<&str>,
) {
    let status = response_status
        .map(|s| s.to_string())
        .unwrap_or_else(|| "ERR".to_string());

    let error_suffix = error
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!(" error={}", s.replace('\n', " ")))
        .unwrap_or_default();

    eprintln!(
        "[sap_adt] http label={} method={} status={} url={}{}",
        label,
        method,
        status,
        url,
        error_suffix
    );
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
    client.set_state(sap);
    client.ensure_started(&bridge_dir, &base_url)?;

    let accept_value = accept.unwrap_or("application/vnd.sap.adt.basic.object.properties+xml, text/plain, text/*, application/xml, text/xml, */*");
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
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            log_adt_http(
                "read_object",
                "GET",
                object_uri,
                status,
                None,
            );

            Ok(AdtReadObjectResult {
                object_uri: object_uri.to_string(),
                content_type: data.get("content_type").and_then(|v| v.as_str()).map(|s| s.to_string()),
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

            log_adt_http(
                "read_object",
                "GET",
                object_uri,
                status,
                Some(&error_text),
            );

            Err(anyhow!(error_text))
        }
    }
}

fn extract_server_etag_from_adt_error(body: &str) -> Option<String> {
    let marker = "object ETag ";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest
        .find(' ')
        .or_else(|| rest.find('<'))
        .unwrap_or(rest.len());
    let value = rest[..end].trim().trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn normalize_if_match_value(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

#[derive(Clone, Debug)]
pub struct AdtLockObjectResult {
    pub object_uri: String,
    pub lock_handle: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
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

#[derive(Clone, Debug)]
enum AdtResolvedRouteFamily {
    SourceMainAtRoot,
    DirectResource,
    RootToSourceMain,
}

#[derive(Clone, Debug)]
struct AdtResolvedRoute {
    route_family: AdtResolvedRouteFamily,
    lock_uri: String,
    write_uri: String,
}

fn split_uri_suffix(uri: &str) -> (String, String) {
    match uri.find(['?', '#']) {
        Some(index) => (uri[..index].to_string(), uri[index..].to_string()),
        None => (uri.to_string(), String::new()),
    }
}

fn resolve_adt_route(object_uri: &str) -> AdtResolvedRoute {
    let trimmed = object_uri.trim();
    if trimmed.is_empty() {
        return AdtResolvedRoute {
            route_family: AdtResolvedRouteFamily::DirectResource,
            lock_uri: String::new(),
            write_uri: String::new(),
        };
    }

    let (base, suffix) = split_uri_suffix(trimmed);
    let normalized_base = base.trim_end_matches('/').to_string();
    let lower = normalized_base.to_ascii_lowercase();

    if lower.ends_with("/source/main") {
        let lock_uri = normalized_base[..normalized_base.len() - "/source/main".len()].to_string();
        return AdtResolvedRoute {
            route_family: AdtResolvedRouteFamily::SourceMainAtRoot,
            lock_uri,
            write_uri: format!("{}{}", normalized_base, suffix),
        };
    }

    if let Some(index) = lower.find("/includes/") {
        return AdtResolvedRoute {
            route_family: AdtResolvedRouteFamily::DirectResource,
            lock_uri: normalized_base[..index].to_string(),
            write_uri: format!("{}{}", normalized_base, suffix),
        };
    }

    let is_root_to_source_main = lower.contains("/programs/programs/")
        || lower.contains("/ddic/ddl/sources/")
        || lower.contains("/oo/classes/");

    if is_root_to_source_main {
        return AdtResolvedRoute {
            route_family: AdtResolvedRouteFamily::RootToSourceMain,
            lock_uri: normalized_base.clone(),
            write_uri: format!("{}/source/main{}", normalized_base, suffix),
        };
    }

    AdtResolvedRoute {
        route_family: AdtResolvedRouteFamily::DirectResource,
        lock_uri: normalized_base.clone(),
        write_uri: format!("{}{}", normalized_base, suffix),
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
    client.set_state(sap);
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

            log_adt_http(
                "lock_object",
                "POST",
                object_uri,
                status,
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

            log_adt_http(
                "lock_object",
                "POST",
                object_uri,
                status,
                Some(&error_text),
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
    client.set_state(sap);
    client.ensure_started(&bridge_dir, &base_url)?;

    let mut headers = serde_json::Map::new();
    if let Some(if_match) = if_match.filter(|s| !s.trim().is_empty()) {
        headers.insert(
            "If-Match".to_string(),
            serde_json::Value::String(normalize_if_match_value(if_match)),
        );
    }

    let content_type_value = content_type.unwrap_or("text/plain; charset=utf-8").to_string();

    let send_update = |client: &mut AdtBridgeClient, headers: &serde_json::Map<String, serde_json::Value>| {
        client.send_json(json!({
            "cmd": "update_object",
            "session_id": session_id,
            "object_uri": object_uri,
            "source": body,
            "content_type": content_type_value,
            "lock_handle": lock_handle,
            "corr_nr": corr_nr,
            "headers": headers
        }))
    };

    let mut resp = send_update(&mut client, &headers);

    if let Err(err) = &resp {
        let error_text = format!("{:#}", err);
        let status = regex::Regex::new(r"\(([0-9]{3})\)")
            .ok()
            .and_then(|re| re.captures(&error_text))
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<u16>().ok());

        if status == Some(412) {
            if let Some(server_etag) = extract_server_etag_from_adt_error(&error_text) {
                headers.insert(
                    "If-Match".to_string(),
                    serde_json::Value::String(server_etag),
                );
                resp = send_update(&mut client, &headers);
            }
        }
    }

    match resp {
        Ok(resp) => {
            let data = resp.get("data").unwrap_or(&resp);
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
            let mut problems = problem_messages_from_json(data.get("problems"));
            if problems.is_empty() {
                problems = extract_adt_problem_messages(&body_text);
            }
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&body_text) {
                    problems.push(message);
                }
            }
            let ok = data
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| status.map(|s| (200..300).contains(&s)).unwrap_or(false) && problems.is_empty());

            log_adt_http(
                "update_object",
                "PUT",
                object_uri,
                status,
                if ok { None } else { Some(&body_text) },
            );

            Ok(AdtUpdateObjectResult {
                status,
                body: body_text,
                problems,
                ok,
            })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = extract_status_from_error_text(&error_text);
            let body = extract_xml_payload_from_error_text(&error_text);
            let mut problems = extract_adt_problem_messages(&body);
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&body) {
                    problems.push(message);
                }
            }
            if problems.is_empty() {
                problems = extract_adt_problem_messages(&error_text);
            }
            if problems.is_empty() {
                problems.push(body.clone());
            }

            log_adt_http(
                "update_object",
                "PUT",
                object_uri,
                status,
                Some(&body),
            );

            Ok(AdtUpdateObjectResult {
                status,
                body,
                problems,
                ok: false,
            })
        }
    }
}

pub fn problem_messages_from_array(items: &[serde_json::Value]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        if !item.is_object() {
            continue;
        }
        let severity = item.get("severity").and_then(|v| v.as_str()).unwrap_or("E");
        let message = item.get("message").and_then(|v| v.as_str()).unwrap_or("").trim();
        if message.is_empty() {
            continue;
        }
        let line = item.get("line").and_then(|v| v.as_u64());
        let column = item.get("column").and_then(|v| v.as_u64());
        let rendered = match (line, column) {
            (Some(line), Some(column)) => format!("{}: line {}, column {}: {}", severity, line, column, message),
            (Some(line), None) => format!("{}: line {}: {}", severity, line, message),
            _ => format!("{}: {}", severity, message),
        };
        out.push(rendered);
    }
    out
}

fn has_source_locations(items: &[String]) -> bool {
    items.iter().any(|item| item.contains("line "))
}

fn merge_problem_message_lists(primary: &[String], secondary: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for item in primary.iter().chain(secondary.iter()) {
        let key = item.trim().to_string();
        if key.is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        out.push(key);
    }
    out
}

pub fn extract_adt_problem_messages(xml: &str) -> Vec<String> {
    let mut out = Vec::new();

    if xml.trim().is_empty() {
        return out;
    }

    let tag_re = Regex::new(r#"(?s)<(?:[A-Za-z0-9_\-]+:)?(?:message|msg|checkMessage)\b([^>]*)>(.*?)</(?:[A-Za-z0-9_\-]+:)?(?:message|msg|checkMessage)>"#).ok();
    let attr_re = Regex::new(r#"([A-Za-z0-9_\-:]+)="([^"]*)""#).ok();
    let strip_re = Regex::new(r#"<[^>]+>"#).ok();

    let decode_xml = |s: &str| {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
    };

    if let Some(tag_re) = tag_re {
        for caps in tag_re.captures_iter(xml) {
            let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let body = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            let mut severity = String::new();
            let mut line = String::new();
            let mut column = String::new();
            let mut code = String::new();

            if let Some(attr_re) = &attr_re {
                for attr_caps in attr_re.captures_iter(attrs) {
                    let name = attr_caps.get(1).map(|m| m.as_str()).unwrap_or_default().to_ascii_lowercase();
                    let value = decode_xml(attr_caps.get(2).map(|m| m.as_str()).unwrap_or_default()).trim().to_string();
                    match name.as_str() {
                        "severity" => severity = value,
                        "line" => line = value,
                        "column" => column = value,
                        "code" => code = value,
                        _ => {}
                    }
                }
            }

            let plain_body = if let Some(strip_re) = &strip_re {
                strip_re.replace_all(body, " ").into_owned()
            } else {
                body.to_string()
            };
            let plain_body = decode_xml(&plain_body).split_whitespace().collect::<Vec<_>>().join(" ");

            let mut parts = Vec::new();
            if !severity.is_empty() {
                parts.push(severity);
            }
            if !code.is_empty() {
                parts.push(code);
            }
            if !line.is_empty() || !column.is_empty() {
                parts.push(match (line.is_empty(), column.is_empty()) {
                    (false, false) => format!("line {}, column {}", line, column),
                    (false, true) => format!("line {}", line),
                    (true, false) => format!("column {}", column),
                    (true, true) => String::new(),
                });
            }
            if !plain_body.is_empty() {
                parts.push(plain_body);
            }

            let message = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(": ");
            if !message.is_empty() {
                out.push(message);
            }
        }
    }

    if out.is_empty() {
        let lowered = xml.to_ascii_lowercase();
        if lowered.contains("<message") || lowered.contains("syntax error") || lowered.contains("activation error") || lowered.contains("error") {
            let fallback = decode_xml(xml).split_whitespace().collect::<Vec<_>>().join(" ");
            if !fallback.is_empty() {
                out.push(fallback);
            }
        }
    }

    out
}

fn problem_messages_from_json(value: Option<&Value>) -> Vec<String> {
    let Some(items) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    items
        .iter()
        .map(|problem| {
            let severity = problem
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let code = problem
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let line = problem.get("line").and_then(|v| v.as_u64());
            let column = problem.get("column").and_then(|v| v.as_u64());
            let message = problem
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();

            let mut parts = Vec::new();
            if !severity.is_empty() {
                parts.push(severity);
            }
            if !code.is_empty() {
                parts.push(code);
            }
            match (line, column) {
                (Some(line), Some(column)) => parts.push(format!("line {}, column {}", line, column)),
                (Some(line), None) => parts.push(format!("line {}", line)),
                (None, Some(column)) => parts.push(format!("column {}", column)),
                (None, None) => {}
            }
            if !message.is_empty() {
                parts.push(message);
            }

            let joined = parts.join(": ");
            if joined.is_empty() {
                problem.to_string()
            } else {
                joined
            }
        })
        .collect()
}

fn extract_xml_payload_from_error_text(error_text: &str) -> String {
    if let Some(idx) = error_text.find("<?xml") {
        return error_text[idx..].trim().to_string();
    }
    if let Some(idx) = error_text.find("<exc:exception") {
        return error_text[idx..].trim().to_string();
    }
    if let Some(idx) = error_text.find("<checkReport") {
        return error_text[idx..].trim().to_string();
    }
    error_text.trim().to_string()
}

fn extract_status_from_error_text(error_text: &str) -> Option<u16> {
    regex::Regex::new(r"\(([0-9]{3})\)")
        .ok()
        .and_then(|re| re.captures(error_text))
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse::<u16>().ok())
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
    client.set_state(sap);
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
                .get("xml")
                .and_then(|v| v.as_str())
                .or_else(|| data.get("body").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));

            let mut problems = problem_messages_from_json(data.get("problems"));
            if problems.is_empty() {
                problems = extract_adt_problem_messages(&body);
            }
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&body) {
                    problems.push(message);
                }
            }

            let status_ok = status.map(|s| (200..300).contains(&s)).unwrap_or(false);
            let bridge_ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(status_ok);
            let ok = bridge_ok && problems.is_empty();

            log_adt_http(
                "syntax_check",
                "POST",
                object_uri,
                status,
                if ok { None } else { Some(&body) },
            );

            Ok(AdtCheckResult {
                status,
                body,
                problems,
                ok,
            })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = extract_status_from_error_text(&error_text);
            let body = extract_xml_payload_from_error_text(&error_text);
            let mut problems = extract_adt_problem_messages(&body);
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&body) {
                    problems.push(message);
                }
            }
            if problems.is_empty() {
                problems = extract_adt_problem_messages(&error_text);
            }
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&error_text) {
                    problems.push(message);
                }
            }
            if problems.is_empty() {
                problems.push(body.clone());
            }

            log_adt_http(
                "syntax_check",
                "POST",
                object_uri,
                status,
                Some(&body),
            );

            Ok(AdtCheckResult {
                status,
                body,
                problems,
                ok: false,
            })
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
    client.set_state(sap);
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
                .get("xml")
                .and_then(|v| v.as_str())
                .or_else(|| data.get("body").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let status = data
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .or_else(|| resp.get("status").and_then(|v| v.as_u64()).map(|v| v as u16));
            let activation_http_failed = status.map(|s| !(200..300).contains(&s)).unwrap_or(false);

            let mut problems = problem_messages_from_json(data.get("problems"));
            let checkruns_xml_problems: Vec<String> = data
                .get("debug")
                .and_then(|v| v.get("checkruns"))
                .and_then(|v| v.get("xml"))
                .and_then(|v| v.as_str())
                .map(extract_adt_problem_messages)
                .unwrap_or_default();
            let checkruns_problems: Vec<String> = data
                .get("debug")
                .and_then(|v| v.get("checkruns"))
                .and_then(|v| v.get("problems"))
                .and_then(|v| v.as_array())
                .map(|items| problem_messages_from_array(items))
                .unwrap_or_default();
            if !activation_http_failed {
                if has_source_locations(&checkruns_problems) {
                    problems = checkruns_problems;
                } else if !checkruns_problems.is_empty() {
                    problems = merge_problem_message_lists(&checkruns_problems, &problems);
                } else if has_source_locations(&checkruns_xml_problems) {
                    problems = checkruns_xml_problems;
                } else if !checkruns_xml_problems.is_empty() {
                    problems = merge_problem_message_lists(&checkruns_xml_problems, &problems);
                }
            }
            if problems.is_empty() {
                problems = extract_adt_problem_messages(&body);
            }
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&body) {
                    problems.push(message);
                }
            }
            let ok = data
                .get("activated")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| status.map(|s| (200..300).contains(&s)).unwrap_or(false) && problems.is_empty());

            log_adt_http(
                "activate_object",
                "POST",
                object_uri,
                status,
                if ok { None } else { Some(&body) },
            );

            Ok(AdtActivateResult { status, body, problems, ok })
        }
        Err(err) => {
            let error_text = format!("{:#}", err);
            let status = extract_status_from_error_text(&error_text);
            let body = extract_xml_payload_from_error_text(&error_text);
            let mut problems = extract_adt_problem_messages(&body);
            if problems.is_empty() {
                if let Some(message) = extract_adt_exception_message(&body) {
                    problems.push(message);
                }
            }
            if problems.is_empty() {
                problems = extract_adt_problem_messages(&error_text);
            }
            if problems.is_empty() {
                problems.push(body.clone());
            }

            log_adt_http(
                "activate_object",
                "POST",
                object_uri,
                status,
                Some(&body),
            );

            Ok(AdtActivateResult {
                status,
                body,
                problems,
                ok: false,
            })
        }
    }
}
