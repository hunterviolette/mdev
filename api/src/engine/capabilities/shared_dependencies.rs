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
    capabilities::registry::{CapabilityContext, CapabilityInvocation, CapabilityInvocationRequest, CapabilityResult},
    runtime_tools::{DependencyEcosystem, DependencyProviderSpec, SharedDependenciesConfig, TrustedDependencyArtifactSpec},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyFingerprint {
    pub provider_id: String,
    pub digest: String,
    pub files: BTreeMap<String, String>,
    pub raw_files: BTreeMap<String, String>,
    pub paths: BTreeMap<String, String>,
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
    pub isolated_seeded: bool,
    pub isolated_path: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
struct SupervisorDependencyScope {
    supervisor_run_id: String,
    work_unit_id: String,
    trusted_root: PathBuf,
    workspace_root: PathBuf,
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let shared_dependencies = resolve_config(ctx, config)?;

    let Some(scope) = load_supervisor_dependency_scope(ctx).await? else {
        tracing::info!(
            run_id = %ctx.run_id,
            step_id = %ctx.step.id,
            repo_ref = %ctx.repo_ref,
            "shared dependencies disabled because workflow has no supervisor work unit"
        );

        return Ok(CapabilityResult {
            ok: true,
            capability: "shared_dependencies".to_string(),
            payload: json!({
                "ok": true,
                "enabled": false,
                "reason": "supervisor_metadata_required",
                "summary": "Shared dependencies are disabled for standalone workflows.",
                "providers": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    };

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

    let explicitly_requested_provider_ids = requested_provider_ids(ctx);
    let requested_provider_ids = if explicitly_requested_provider_ids.is_empty() {
        infer_provider_ids(ctx, &shared_dependencies.providers)
    } else {
        explicitly_requested_provider_ids
    };
    let requested_provider_ids = requested_provider_ids
        .into_iter()
        .filter(|requested_id| {
            shared_dependencies.providers.iter().any(|provider| {
                matches!(provider.ecosystem, DependencyEcosystem::Node)
                    && provider.id == *requested_id
            })
        })
        .collect::<Vec<_>>();

    if requested_provider_ids.is_empty() {
        return Ok(CapabilityResult {
            ok: true,
            capability: "shared_dependencies".to_string(),
            payload: json!({
                "ok": true,
                "enabled": true,
                "skipped": true,
                "reason": "no_applicable_dependency_providers",
                "providers": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }
    let workspace_root = scope.workspace_root;
    let trusted_root = scope.trusted_root;
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

    let issues = dependency_issues(&resolved);
    let has_mismatch = !issues.is_empty();

    let follow_ups = if has_mismatch {
        CapabilityInvocationRequest::One(CapabilityInvocation {
            capability: "operator_checkpoint".to_string(),
            config: json!({
                "recommended_disposition": "pause_error",
                "available_dispositions": [
                    "continue_auto",
                    "select_stage",
                    "pause_error"
                ]
            }),
        })
    } else {
        CapabilityInvocationRequest::None
    };

    Ok(CapabilityResult {
        ok: !has_mismatch,
        capability: "shared_dependencies".to_string(),
        payload: json!({
            "ok": !has_mismatch,
            "enabled": true,
            "scope": {
                "supervisor_run_id": scope.supervisor_run_id,
                "work_unit_id": scope.work_unit_id,
                "trusted_root": trusted_root,
                "workspace_root": workspace_root
            },
            "summary": if has_mismatch {
                format!(
                    "Shared dependency validation found {} provider mismatch{}. No dependency copy was created.",
                    issues.len(),
                    if issues.len() == 1 { "" } else { "es" }
                )
            } else {
                "Shared dependency validation succeeded. The shard uses the trusted Node dependency layer.".to_string()
            },
            "issues": issues,
            "providers": resolved
        }),
        follow_ups,
    })
}

fn dependency_issues(providers: &[ResolvedDependencyProvider]) -> Vec<Value> {
    providers
        .iter()
        .filter(|provider| !provider.manifest_match)
        .map(|provider| {
            let mut manifest_names = provider
                .trusted_fingerprint
                .files
                .keys()
                .chain(provider.environment_fingerprint.files.keys())
                .cloned()
                .collect::<Vec<_>>();
            manifest_names.sort();
            manifest_names.dedup();

            let changed_manifests = manifest_names
                .into_iter()
                .filter_map(|manifest| {
                    let trusted_semantic_hash = provider.trusted_fingerprint.files.get(&manifest);
                    let workspace_semantic_hash = provider.environment_fingerprint.files.get(&manifest);

                    if trusted_semantic_hash == workspace_semantic_hash {
                        return None;
                    }

                    Some(json!({
                        "manifest": manifest,
                        "change": match (trusted_semantic_hash, workspace_semantic_hash) {
                            (Some(_), Some(_)) => "modified",
                            (Some(_), None) => "missing_from_workspace",
                            (None, Some(_)) => "missing_from_trusted_root",
                            (None, None) => "unknown"
                        },
                        "trusted": {
                            "path": provider.trusted_fingerprint.paths.get(&manifest),
                            "semantic_hash": trusted_semantic_hash,
                            "raw_hash": provider.trusted_fingerprint.raw_files.get(&manifest)
                        },
                        "workspace": {
                            "path": provider.environment_fingerprint.paths.get(&manifest),
                            "semantic_hash": workspace_semantic_hash,
                            "raw_hash": provider.environment_fingerprint.raw_files.get(&manifest)
                        }
                    }))
                })
                .collect::<Vec<_>>();

            let changed_paths = changed_manifests
                .iter()
                .filter_map(|item| item.get("manifest").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");

            json!({
                "provider_id": provider.provider_id,
                "ecosystem": provider.ecosystem,
                "status": provider.status,
                "summary": if changed_paths.is_empty() {
                    format!("Provider '{}' does not match the trusted dependency snapshot.", provider.provider_id)
                } else {
                    format!("Provider '{}' has semantic manifest changes in: {}.", provider.provider_id, changed_paths)
                },
                "changed_manifests": changed_manifests,
                "action": "operator_checkpoint",
                "compile_blocked": true,
                "dependency_copy_created": false
            })
        })
        .collect()
}

fn resolve_config(ctx: &CapabilityContext<'_>, config: Value) -> Result<SharedDependenciesConfig> {
    if !config.is_null() && config != json!({}) {
        return serde_json::from_value(config).context("invalid shared dependencies capability configuration");
    }

    let value = ctx
        .local_state
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("shared_dependencies"))
        .cloned()
        .or_else(|| {
            ctx.local_state
                .get("global_state")
                .and_then(|global| global.get("capabilities"))
                .and_then(|capabilities| capabilities.get("shared_dependencies"))
                .cloned()
        })
        .or_else(|| ctx.local_state.get("shared_dependencies").cloned())
        .or_else(|| {
            ctx.local_state
                .get("global_state")
                .and_then(|global| global.get("shared_dependencies"))
                .cloned()
        })
        .unwrap_or_else(|| json!({}));

    serde_json::from_value(value).context("invalid shared dependencies workflow configuration")
}

fn infer_provider_ids(
    ctx: &CapabilityContext<'_>,
    providers: &[DependencyProviderSpec],
) -> Vec<String> {
    let command_text = command_text_for_step(ctx);

    providers
        .iter()
        .filter(|provider| provider_matches_command_text(provider, &command_text))
        .map(|provider| provider.id.clone())
        .collect()
}

fn command_text_for_step(ctx: &CapabilityContext<'_>) -> String {
    let mut values = Vec::new();

    if let Ok(value) = serde_json::to_string(&ctx.step.execution) {
        values.push(value);
    }

    if let Ok(value) = serde_json::to_string(&ctx.step.execution_logic) {
        values.push(value);
    }

    if let Some(execution) = ctx.local_state.get("execution") {
        if let Ok(value) = serde_json::to_string(execution) {
            values.push(value);
        }
    }

    values
        .join(" ")
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn provider_matches_command_text(
    provider: &DependencyProviderSpec,
    command_text: &str,
) -> bool {
    let ecosystem_matches = match provider.ecosystem {
        DependencyEcosystem::Cargo => {
            command_text.contains("cargo ")
                || command_text.contains("cargo\"")
                || command_text.contains("cargo.exe")
        }
        DependencyEcosystem::Node => {
            command_text.contains("npm ")
                || command_text.contains("npm.cmd")
                || command_text.contains("pnpm ")
                || command_text.contains("yarn ")
                || command_text.contains("node ")
                || command_text.contains("node.exe")
        }
    };

    if !ecosystem_matches {
        return false;
    }

    let root = provider
        .root
        .trim()
        .trim_matches('/')
        .replace('\\', "/")
        .to_ascii_lowercase();

    if root.is_empty() || root == "." {
        return true;
    }

    command_text.contains(&format!("cd {} ", root))
        || command_text.contains(&format!("cd {}/", root))
        || command_text.contains(&format!("\"working_directory\":\"{}\"", root))
        || command_text.contains(&format!("\"working_directory\": \"{}\"", root))
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
        _ => Vec::new(),
    }
}

async fn load_supervisor_dependency_scope(
    ctx: &CapabilityContext<'_>,
) -> Result<Option<SupervisorDependencyScope>> {
    let row = sqlx::query_as::<_, (String, String, String, String)>(
        r#"
        SELECT
            sw.id,
            sw.supervisor_run_id,
            sr.root_repo_path,
            sw.workspace_path
        FROM supervisor_work_units sw
        INNER JOIN supervisor_runs sr
            ON sr.id = sw.supervisor_run_id
        WHERE sw.workflow_run_id = ?
          AND TRIM(COALESCE(sr.root_repo_path, '')) != ''
          AND TRIM(COALESCE(sw.workspace_path, '')) != ''
        ORDER BY sw.updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.run_id.to_string())
    .fetch_optional(&ctx.state.db)
    .await?;

    let Some((work_unit_id, supervisor_run_id, trusted_root, workspace_root)) = row else {
        return Ok(None);
    };

    let trusted_root = PathBuf::from(trusted_root)
        .canonicalize()
        .context("failed to resolve supervisor trusted repository root")?;
    let workspace_root = PathBuf::from(workspace_root)
        .canonicalize()
        .context("failed to resolve supervisor workflow workspace")?;

    if trusted_root == workspace_root {
        bail!(
            "supervisor dependency scope resolves trusted root and workspace to the same path: {}",
            trusted_root.display()
        );
    }

    let supervisor_workspace_root = trusted_root.join(".mdev");
    if !workspace_root.starts_with(&supervisor_workspace_root) {
        bail!(
            "supervisor workspace '{}' is outside the trusted repository's .mdev directory '{}'",
            workspace_root.display(),
            supervisor_workspace_root.display()
        );
    }

    Ok(Some(SupervisorDependencyScope {
        supervisor_run_id,
        work_unit_id,
        trusted_root,
        workspace_root,
    }))
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted = map
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();

            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonicalize_json).collect())
        }
        other => other,
    }
}

fn semantic_manifest_digest(path: &Path, contents: &[u8]) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();

    if extension.eq_ignore_ascii_case("json") {
        let value: Value = serde_json::from_slice(contents)
            .with_context(|| format!("failed to parse dependency manifest JSON {}", path.display()))?;
        let canonical = serde_json::to_vec(&canonicalize_json(value))
            .with_context(|| format!("failed to canonicalize dependency manifest {}", path.display()))?;
        return Ok(format!("sha256:{:x}", Sha256::digest(canonical)));
    }

    Ok(format!("sha256:{:x}", Sha256::digest(contents)))
}

pub fn fingerprint_provider(
    root: &Path,
    provider: &DependencyProviderSpec,
) -> Result<DependencyFingerprint> {
    let provider_root = resolve_existing_under_root(root, provider.root.as_str())?;
    let mut files = BTreeMap::new();
    let mut raw_files = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut aggregate = Sha256::new();

    aggregate.update(provider.id.as_bytes());
    aggregate.update(format!("{:?}", provider.ecosystem).as_bytes());

    for manifest in &provider.manifests {
        let manifest_path = resolve_existing_under_root(&provider_root, manifest.as_str())?;
        let contents = fs::read(&manifest_path)
            .with_context(|| format!("failed to read dependency manifest {}", manifest_path.display()))?;
        let raw_digest = format!("sha256:{:x}", Sha256::digest(&contents));
        let semantic_digest = semantic_manifest_digest(manifest_path.as_path(), &contents)?;

        aggregate.update(manifest.as_bytes());
        aggregate.update(semantic_digest.as_bytes());
        files.insert(manifest.clone(), semantic_digest);
        raw_files.insert(manifest.clone(), raw_digest);
        paths.insert(manifest.clone(), manifest_path.to_string_lossy().to_string());
    }

    Ok(DependencyFingerprint {
        provider_id: provider.id.clone(),
        digest: format!("sha256:{:x}", aggregate.finalize()),
        files,
        raw_files,
        paths,
    })
}

fn effective_trusted_artifact(
    provider: &DependencyProviderSpec,
) -> TrustedDependencyArtifactSpec {
    provider.trusted_artifact.clone().unwrap_or_else(|| {
        match provider.ecosystem {
            DependencyEcosystem::Node => TrustedDependencyArtifactSpec::NodeModules {
                path: "node_modules".to_string(),
                read_only: true,
            },
            DependencyEcosystem::Cargo => TrustedDependencyArtifactSpec::Cargo {
                cargo_home: Some(".cargo".to_string()),
                target_directory: Some("target".to_string()),
                share_target: true,
            },
        }
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

    if !manifest_match {
        tracing::warn!(
            provider_id = %provider.id,
            trusted_root = %trusted_root.display(),
            workspace_root = %environment_root.display(),
            trusted_aggregate_hash = %trusted_fingerprint.digest,
            workspace_aggregate_hash = %environment_fingerprint.digest,
            "shared dependency manifest fingerprint mismatch"
        );

        let mut manifest_names = trusted_fingerprint
            .files
            .keys()
            .chain(environment_fingerprint.files.keys())
            .cloned()
            .collect::<Vec<_>>();
        manifest_names.sort();
        manifest_names.dedup();

        for manifest in manifest_names {
            let trusted_semantic_hash = trusted_fingerprint.files.get(&manifest);
            let workspace_semantic_hash = environment_fingerprint.files.get(&manifest);

            if trusted_semantic_hash == workspace_semantic_hash {
                continue;
            }

            tracing::warn!(
                provider_id = %provider.id,
                manifest = %manifest,
                trusted_path = ?trusted_fingerprint.paths.get(&manifest),
                workspace_path = ?environment_fingerprint.paths.get(&manifest),
                trusted_semantic_hash = ?trusted_semantic_hash,
                workspace_semantic_hash = ?workspace_semantic_hash,
                trusted_raw_hash = ?trusted_fingerprint.raw_files.get(&manifest),
                workspace_raw_hash = ?environment_fingerprint.raw_files.get(&manifest),
                "shared dependency manifest differs from trusted root"
            );
        }
    }
    let mut environment = BTreeMap::new();
    let mut mounts = Vec::new();
    let mut isolated_seeded = false;
    let mut isolated_path = None;
    let trusted_provider_root = resolve_existing_under_root(trusted_root, provider.root.as_str())?;
    let environment_provider_root = resolve_existing_under_root(environment_root, provider.root.as_str())?;
    let trusted_artifact = effective_trusted_artifact(provider);

    if manifest_match {
        match &trusted_artifact {
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
            "manifest_mismatch_operator_required"
        }
        .to_string(),
        manifest_match,
        trusted_fingerprint,
        environment_fingerprint,
        environment,
        mounts,
        isolated_seeded,
        isolated_path,
        warning: (!manifest_match).then(|| {
            "Dependency manifests differ semantically from the trusted root dependency layer. No private dependency copy was created; operator approval is required before isolation.".to_string()
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

fn remove_dependency_target(target: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect dependency target {}", target.display()))
        }
    };

    if metadata.file_type().is_symlink() {
        remove_directory_link(target)?;
    } else if metadata.is_dir() {
        fs::remove_dir_all(target)
            .with_context(|| format!("failed to remove dependency directory {}", target.display()))?;
    } else {
        fs::remove_file(target)
            .with_context(|| format!("failed to remove dependency file {}", target.display()))?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn remove_directory_link(target: &Path) -> Result<()> {
    fs::remove_dir(target)
        .with_context(|| format!("failed to remove dependency junction {}", target.display()))
}

#[cfg(not(target_os = "windows"))]
fn remove_directory_link(target: &Path) -> Result<()> {
    fs::remove_file(target)
        .with_context(|| format!("failed to remove dependency symlink {}", target.display()))
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
