use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::engine::{
    capabilities::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
    runtime_tools::{DependencyEcosystem, DependencyProviderSpec, SharedDependenciesConfig, TrustedDependencyArtifactSpec},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyFingerprint {
    pub provider_id: String,
    pub digest: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyMount {
    pub source: String,
    pub target: String,
    pub strategy: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedDependencyProvider {
    pub provider_id: String,
    pub ecosystem: DependencyEcosystem,
    pub status: String,
    pub manifest_match: bool,
    pub trusted_fingerprint: DependencyFingerprint,
    pub environment_fingerprint: DependencyFingerprint,
    pub environment: BTreeMap<String, String>,
    pub mounts: Vec<DependencyMount>,
    pub warning: Option<String>,
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let shared_dependencies = resolve_config(ctx, config)?;

    if !shared_dependencies.enabled {
        return Ok(CapabilityResult {
            ok: true,
            capability: "shared_dependencies".to_string(),
            payload: json!({
                "ok": true,
                "enabled": false,
                "providers": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let requested_provider_ids = requested_provider_ids(ctx);

    if requested_provider_ids.is_empty() {
        return Ok(CapabilityResult {
            ok: true,
            capability: "shared_dependencies".to_string(),
            payload: json!({
                "ok": true,
                "enabled": true,
                "skipped": true,
                "reason": "no_dependency_providers_selected",
                "providers": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }
    let workspace_root = ctx
        .local_state
        .get("resources")
        .and_then(|resources| resources.get("repo"))
        .and_then(|repo| repo.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(ctx.repo_ref));
    let trusted_root = trusted_root(ctx, workspace_root.as_path())?;
    let mut resolved = Vec::new();

    for provider in shared_dependencies.providers.iter().filter(|provider| {
        requested_provider_ids.iter().any(|id| id == &provider.id)
    }) {
        let resolved_provider = resolve_trusted_provider(
            trusted_root.as_path(),
            workspace_root.as_path(),
            provider,
        )?;

        if resolved_provider.manifest_match {
            materialize_dependency_mounts(&resolved_provider.mounts)?;
        }

        resolved.push(resolved_provider);
    }

    let has_mismatch = resolved.iter().any(|provider| !provider.manifest_match);

    Ok(CapabilityResult {
        ok: !has_mismatch,
        capability: "shared_dependencies".to_string(),
        payload: json!({
            "ok": !has_mismatch,
            "enabled": true,
            "requires_operator_checkpoint": has_mismatch,
            "providers": resolved
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_config(ctx: &CapabilityContext<'_>, config: Value) -> Result<SharedDependenciesConfig> {
    if !config.is_null() && config != json!({}) {
        return serde_json::from_value(config).context("invalid shared dependencies capability configuration");
    }

    let value = ctx
        .local_state
        .get("shared_dependencies")
        .cloned()
        .or_else(|| {
            ctx.local_state
                .get("global_state")
                .and_then(|global| global.get("shared_dependencies"))
                .cloned()
        })
        .unwrap_or_else(|| json!({}));

    serde_json::from_value(value).context("invalid shared dependencies workflow configuration")
}

fn requested_provider_ids(ctx: &CapabilityContext<'_>) -> Vec<String> {
    match ctx.step.step_type.as_str() {
        "compile" => ctx
            .step
            .execution
            .compile
            .as_ref()
            .map(|compile| compile.dependency_providers.clone())
            .unwrap_or_default(),
        "qa" => ctx
            .step
            .execution
            .qa
            .as_ref()
            .map(|qa| qa.dependency_providers.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn trusted_root(
    ctx: &CapabilityContext<'_>,
    workspace_root: &Path,
) -> Result<PathBuf> {
    let configured = ctx
        .local_state
        .get("resources")
        .and_then(|resources| resources.get("trusted_root_repo"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            ctx.local_state
                .get("global_state")
                .and_then(|global| global.get("resources"))
                .and_then(|resources| resources.get("trusted_root_repo"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            std::env::var("MDEV_TRUSTED_ROOT_REPO")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        });

    let trusted = match configured {
        Some(path) => path,
        None => discover_running_repository_root(workspace_root)?,
    };

    let trusted = trusted
        .canonicalize()
        .with_context(|| format!("failed to resolve trusted dependency root {}", trusted.display()))?;
    let workspace = workspace_root
        .canonicalize()
        .with_context(|| format!("failed to resolve workflow workspace {}", workspace_root.display()))?;

    if trusted == workspace {
        bail!(
            "trusted dependency root resolved to the workflow workspace '{}'. Configure resources.trusted_root_repo or MDEV_TRUSTED_ROOT_REPO with the parent repository path",
            workspace.display()
        );
    }

    Ok(trusted)
}

fn discover_running_repository_root(workspace_root: &Path) -> Result<PathBuf> {
    let workspace = workspace_root.canonicalize().ok();
    let current = std::env::current_dir().context("failed to determine API working directory")?;

    for candidate in current.ancestors() {
        if workspace
            .as_ref()
            .map(|workspace| candidate == workspace.as_path())
            .unwrap_or(false)
        {
            continue;
        }

        if candidate.join("package.json").is_file()
            && candidate.join("package-lock.json").is_file()
            && candidate.join("node_modules").is_dir()
            && candidate.join("api").join("Cargo.toml").is_file()
        {
            return Ok(candidate.to_path_buf());
        }
    }

    bail!(
        "unable to discover the trusted dependency repository from API working directory '{}'. Configure resources.trusted_root_repo or MDEV_TRUSTED_ROOT_REPO",
        current.display()
    )
}

pub fn fingerprint_provider(
    root: &Path,
    provider: &DependencyProviderSpec,
) -> Result<DependencyFingerprint> {
    let provider_root = resolve_existing_under_root(root, provider.root.as_str())?;
    let mut files = BTreeMap::new();
    let mut aggregate = Sha256::new();

    aggregate.update(provider.id.as_bytes());
    aggregate.update(format!("{:?}", provider.ecosystem).as_bytes());

    for manifest in &provider.manifests {
        let manifest_path = resolve_existing_under_root(&provider_root, manifest.as_str())?;
        let contents = fs::read(&manifest_path)
            .with_context(|| format!("failed to read dependency manifest {}", manifest_path.display()))?;
        let digest = format!("{:x}", Sha256::digest(&contents));
        aggregate.update(manifest.as_bytes());
        aggregate.update(digest.as_bytes());
        files.insert(manifest.clone(), digest);
    }

    Ok(DependencyFingerprint {
        provider_id: provider.id.clone(),
        digest: format!("sha256:{:x}", aggregate.finalize()),
        files,
    })
}

pub fn resolve_trusted_provider(
    trusted_root: &Path,
    environment_root: &Path,
    provider: &DependencyProviderSpec,
) -> Result<ResolvedDependencyProvider> {
    let trusted_root_canonical = trusted_root
        .canonicalize()
        .with_context(|| format!("failed to resolve trusted dependency root {}", trusted_root.display()))?;
    let environment_root_canonical = environment_root
        .canonicalize()
        .with_context(|| format!("failed to resolve dependency workspace {}", environment_root.display()))?;

    if trusted_root_canonical == environment_root_canonical {
        bail!(
            "shared dependency provider '{}' cannot reuse dependencies because its trusted root and workflow workspace are both '{}'",
            provider.id,
            environment_root_canonical.display()
        );
    }

    let trusted_fingerprint = fingerprint_provider(trusted_root, provider)?;
    let environment_fingerprint = fingerprint_provider(environment_root, provider)?;
    let manifest_match = trusted_fingerprint.digest == environment_fingerprint.digest;
    let mut environment = BTreeMap::new();
    let mut mounts = Vec::new();

    if manifest_match {
        let trusted_provider_root = resolve_existing_under_root(trusted_root, provider.root.as_str())?;
        let environment_provider_root = resolve_existing_under_root(environment_root, provider.root.as_str())?;

        match &provider.trusted_artifact {
            TrustedDependencyArtifactSpec::NodeModules { path, read_only } => {
                mounts.push(DependencyMount {
                    source: resolve_existing_under_root(&trusted_provider_root, path)?
                        .to_string_lossy()
                        .to_string(),
                    target: resolve_target_under_root(&environment_provider_root, path)?
                        .to_string_lossy()
                        .to_string(),
                    strategy: "directory_link".to_string(),
                    read_only: *read_only,
                });
            }
            TrustedDependencyArtifactSpec::Cargo {
                cargo_home,
                target_directory,
                share_target,
            } => {
                if let Some(cargo_home) = cargo_home {
                    environment.insert(
                        "CARGO_HOME".to_string(),
                        resolve_existing_under_root(&trusted_provider_root, cargo_home)?
                            .to_string_lossy()
                            .to_string(),
                    );
                }

                if let Some(target_directory) = target_directory {
                    let target_root = if *share_target {
                        &trusted_provider_root
                    } else {
                        &environment_provider_root
                    };
                    environment.insert(
                        "CARGO_TARGET_DIR".to_string(),
                        resolve_target_under_root(target_root, target_directory)?
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }
    }

    Ok(ResolvedDependencyProvider {
        provider_id: provider.id.clone(),
        ecosystem: provider.ecosystem.clone(),
        status: if manifest_match {
            "trusted_reuse"
        } else {
            "manifest_mismatch"
        }
        .to_string(),
        manifest_match,
        trusted_fingerprint,
        environment_fingerprint,
        environment,
        mounts,
        warning: (!manifest_match).then(|| {
            "Dependency manifests differ from the trusted root dependency layer.".to_string()
        }),
    })
}

fn materialize_dependency_mounts(mounts: &[DependencyMount]) -> Result<()> {
    for mount in mounts {
        match mount.strategy.as_str() {
            "directory_link" => create_directory_link(
                Path::new(mount.source.as_str()),
                Path::new(mount.target.as_str()),
            )?,
            other => bail!("unsupported shared dependency mount strategy '{}'", other),
        }
    }

    Ok(())
}

fn create_directory_link(source: &Path, target: &Path) -> Result<()> {
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve trusted dependency artifact {}", source.display()))?;

    if target.exists() {
        if target
            .canonicalize()
            .map(|existing| existing == source)
            .unwrap_or(false)
        {
            return Ok(());
        }

        let metadata = fs::symlink_metadata(target)
            .with_context(|| format!("failed to inspect dependency target {}", target.display()))?;

        if metadata.file_type().is_symlink() {
            fs::remove_file(target)
                .with_context(|| format!("failed to replace dependency link {}", target.display()))?;
        } else if metadata.is_dir()
            && fs::read_dir(target)
                .with_context(|| format!("failed to inspect dependency directory {}", target.display()))?
                .next()
                .is_none()
        {
            fs::remove_dir(target)
                .with_context(|| format!("failed to replace empty dependency directory {}", target.display()))?;
        } else {
            bail!(
                "dependency target '{}' already exists and is not linked to '{}'",
                target.display(),
                source.display()
            );
        }
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dependency target parent {}", parent.display()))?;
    }

    create_platform_directory_link(source.as_path(), target)?;

    let linked = target
        .canonicalize()
        .with_context(|| format!("failed to resolve created dependency link {}", target.display()))?;

    if linked != source {
        bail!(
            "dependency link '{}' resolved to '{}' instead of '{}'",
            target.display(),
            linked.display(),
            source.display()
        );
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn create_platform_directory_link(source: &Path, target: &Path) -> Result<()> {
    let output = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(target)
        .arg(source)
        .output()
        .with_context(|| {
            format!(
                "failed to create dependency junction {} -> {}",
                target.display(),
                source.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "failed to create dependency junction {} -> {}: {}",
            target.display(),
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn create_platform_directory_link(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target).with_context(|| {
        format!(
            "failed to create dependency symlink {} -> {}",
            target.display(),
            source.display()
        )
    })
}

fn normalized_relative_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value.trim());

    if path.is_absolute() {
        bail!("dependency paths must be relative");
    }

    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("dependency paths may not escape their root");
    }

    Ok(if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path
    })
}

fn resolve_existing_under_root(root: &Path, relative: &str) -> Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve dependency root {}", root.display()))?;
    let relative = normalized_relative_path(relative)?;
    let resolved = canonical_root
        .join(relative)
        .canonicalize()
        .context("failed to resolve dependency path")?;

    if !resolved.starts_with(&canonical_root) {
        bail!("dependency path escapes its root");
    }

    Ok(resolved)
}

fn resolve_target_under_root(root: &Path, relative: &str) -> Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve dependency root {}", root.display()))?;
    let relative = normalized_relative_path(relative)?;
    let target = canonical_root.join(relative);

    if !target.starts_with(&canonical_root) {
        bail!("dependency target escapes its root");
    }

    Ok(target)
}
