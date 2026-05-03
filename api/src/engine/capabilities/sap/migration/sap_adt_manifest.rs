use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapAdtObjectManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub metadata_uri: String,
    #[serde(default)]
    pub object_uri: Option<String>,
    #[serde(default)]
    pub object_name: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(skip_serializing, default)]
    pub etag: Option<String>,
    #[serde(skip_serializing, default)]
    pub metadata_xml: String,
    #[serde(default)]
    pub resources: Vec<SapAdtManifestResource>,
    #[serde(skip_serializing, default)]
    pub documents: Vec<SapAdtManifestDocument>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapAdtManifestResource {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(skip_serializing, default)]
    pub etag: Option<String>,
    #[serde(skip_serializing, default)]
    pub lock_handle: Option<String>,
    #[serde(skip_serializing, default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub readable: bool,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub activatable: bool,
    #[serde(default)]
    pub role: String,
    #[serde(skip_serializing, default)]
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapAdtManifestDocument {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(skip_serializing, default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub path: String,
    #[serde(skip_serializing, default)]
    pub body: String,
}

fn default_schema_version() -> u32 {
    1
}

impl SapAdtObjectManifest {
    pub fn primary_resource_id(&self) -> Option<String> {
        self.resources
            .iter()
            .find(|resource| resource.editable)
            .or_else(|| self.resources.iter().find(|resource| resource.readable))
            .or_else(|| self.resources.first())
            .map(|resource| resource.id.clone())
    }

    pub fn selected_resource(&self, selected_resource_id: Option<&str>) -> Option<&SapAdtManifestResource> {
        if let Some(id) = selected_resource_id {
            if let Some(resource) = self.resources.iter().find(|resource| resource.id == id) {
                return Some(resource);
            }
        }

        let primary = self.primary_resource_id()?;
        self.resources.iter().find(|resource| resource.id == primary)
    }

    pub fn selected_resource_mut(
        &mut self,
        selected_resource_id: Option<&str>,
    ) -> Option<&mut SapAdtManifestResource> {
        let resolved_id = if let Some(id) = selected_resource_id {
            Some(id.to_string())
        } else {
            self.primary_resource_id()
        }?;

        self.resources.iter_mut().find(|resource| resource.id == resolved_id)
    }
}
