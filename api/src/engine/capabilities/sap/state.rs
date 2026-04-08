use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::migration::sap_adt_manifest::SapAdtObjectManifest;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecuteLoopTurnResult {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub browser_session_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BrowserProbeResult {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub browser_session_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SapAdtTemplateLink {
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub template: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SapAdtDiscoveryCollection {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub category_term: Option<String>,
    #[serde(default)]
    pub category_scheme: Option<String>,
    #[serde(default)]
    pub accepts: Vec<String>,
    #[serde(default)]
    pub template_links: Vec<SapAdtTemplateLink>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SapAdtDiscoveryState {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub collections: Vec<SapAdtDiscoveryCollection>,
    #[serde(default)]
    pub links: Vec<SapAdtTemplateLink>,
    #[serde(default)]
    pub xml: String,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub package_collection_href: Option<String>,
    #[serde(default)]
    pub package_tree_href: Option<String>,
    #[serde(default)]
    pub repository_search_href: Option<String>,
    #[serde(default)]
    pub repository_search_template: Option<String>,
    #[serde(default)]
    pub object_types_href: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SapAdtObjectSummary {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SapAdtState {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth_type: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub authorization: String,
    #[serde(default)]
    pub cookie_header: Option<String>,
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub bridge_dir: String,
    #[serde(default)]
    pub browser_bridge_dir: String,
    #[serde(default)]
    pub browser_user_data_dir: String,
    #[serde(default)]
    pub browser_channel: String,
    #[serde(default)]
    pub browser_session_id: Option<String>,
    #[serde(default)]
    pub browser_conversation_id: Option<String>,
    #[serde(default)]
    pub package_query: String,
    #[serde(default)]
    pub include_subpackages: bool,
    #[serde(default)]
    pub package_tree_xml: String,
    #[serde(default)]
    pub package_objects: Vec<SapAdtObjectSummary>,
    #[serde(default)]
    pub import_selected_object_uris: HashSet<String>,
    #[serde(default)]
    pub selected_object_uri: Option<String>,
    #[serde(default)]
    pub selected_object_metadata_uri: Option<String>,
    #[serde(default)]
    pub selected_object_name: Option<String>,
    #[serde(default)]
    pub selected_object_type: Option<String>,
    #[serde(default)]
    pub selected_object_content: String,
    #[serde(default)]
    pub selected_object_content_type: Option<String>,
    #[serde(default)]
    pub selected_object_headers: Vec<(String, String)>,
    #[serde(default)]
    pub selected_object_metadata: String,
    #[serde(default)]
    pub selected_object_metadata_content_type: Option<String>,
    #[serde(default)]
    pub selected_manifest: Option<SapAdtObjectManifest>,
    #[serde(default)]
    pub selected_resource_id: Option<String>,
    #[serde(default)]
    pub clone_target_path: String,
    #[serde(default)]
    pub discovery: Option<SapAdtDiscoveryState>,
    #[serde(default)]
    pub discovery_xml: String,
    #[serde(default)]
    pub discovery_url: Option<String>,
    #[serde(default)]
    pub adt_session_id: Option<String>,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_status: Option<String>,
}
