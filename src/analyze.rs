use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::app::state::WORKTREE_REF;
use crate::git;
use crate::model::{AnalysisResult, DirNode, DirStats, FileInfo, FileRow, StatAgg};

fn decode_lines(blob: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(blob)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn ext_of(p: &str) -> String {
    std::path::Path::new(p)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn count_loc(blob: &[u8]) -> u64 {
    // count lines = count '\n' + 1 if non-empty
    if blob.is_empty() {
        0
    } else {
        (blob.iter().filter(|b| **b == b'\n').count() as u64) + 1
    }
}

fn is_binary(blob: &[u8]) -> bool {
    blob.iter().any(|&b| b == 0)
}

fn normalize_repo_rel(s: &str) -> String {
    let mut out = s.replace('\\', "/");
    while let Some(rest) = out.strip_prefix("./") {
        out = rest.to_string();
    }
    while let Some(rest) = out.strip_prefix('/') {
        out = rest.to_string();
    }
    out
}

fn top_level_dir(path: &str) -> String {
    let p = normalize_repo_rel(path);
    if let Some((first, _rest)) = p.split_once('/') {
        if first.is_empty() {
            ".".to_string()
        } else {
            first.to_string()
        }
    } else if p.is_empty() {
        ".".to_string()
    } else {
        ".".to_string()
    }
}

fn file_row(i: &FileInfo) -> FileRow {
    let name = std::path::Path::new(&i.path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&i.path)
        .to_string();

    FileRow {
        name,
        full_path: i.path.clone(),
        loc_display: if i.is_text {
            i.loc.map(|n| n.to_string()).unwrap_or_else(|| "0".to_string())
        } else {
            "(bin)".to_string()
        },
        ext: i.ext.clone(),
    }
}

pub fn analyze_repo(repo: &Path, git_ref: &str, exclude: &[Regex], _max_exts: usize) -> Result<AnalysisResult> {
    let use_worktree = git_ref == WORKTREE_REF;

    // List files
    let raw_list: Vec<u8> = if use_worktree {
        // tracked + untracked
        let tracked = git::run_git(repo, &["ls-files"])?;
        let untracked = git::run_git(repo, &["ls-files", "--others", "--exclude-standard"])?;
        let mut combined = Vec::new();
        combined.extend_from_slice(&tracked);
        combined.push(b'\n');
        combined.extend_from_slice(&untracked);
        combined
    } else {
        git::run_git(repo, &["ls-tree", "-r", "--name-only", git_ref])?
    };

    let mut files: Vec<String> = decode_lines(&raw_list)
        .into_iter()
        .map(|p| normalize_repo_rel(&p))
        .filter(|f| !exclude.iter().any(|rx| rx.is_match(f)))
        .collect();
    files.sort();
    files.dedup();

    let mut infos: Vec<FileInfo> = Vec::with_capacity(files.len());
    let mut skipped_bin = 0u64;

    for f in &files {
        let ext = ext_of(f);

        let blob: Vec<u8> = if use_worktree {
            // WORKTREE: read from disk
            match git::read_worktree_file(repo, f) {
                Ok(b) => b,
                Err(_) => Vec::new(), // ignore missing/locked files gracefully
            }
        } else {
            // ref: use git show <ref>:<path>
            let spec = format!("{}:{}", git_ref, f);
            let (code, blob, _stderr) = git::run_git_allow_fail(repo, &["show", &spec])?;
            if code != 0 { Vec::new() } else { blob }
        };

        if blob.is_empty() || is_binary(&blob) {
            skipped_bin += 1;
            infos.push(FileInfo {
                path: f.clone(),
                ext,
                is_text: false,
                loc: None,
            });
            continue;
        }

        infos.push(FileInfo {
            path: f.clone(),
            ext,
            is_text: true,
            loc: Some(count_loc(&blob)),
        });
    }

    // Stats
    let mut overall = StatAgg::default();
    let mut by_ext: HashMap<String, StatAgg> = HashMap::new();

    for i in &infos {
        overall.add(i);
        by_ext.entry(i.ext.clone()).or_default().add(i);
    }

    let mut ext_stats: Vec<(String, StatAgg)> = by_ext.into_iter().collect();
    ext_stats.sort_by(|(e1, a1), (e2, a2)| a2.loc_total.cmp(&a1.loc_total).then_with(|| e1.cmp(e2)));

    let root = build_tree(&infos);

    Ok(AnalysisResult {
        repo: repo.to_path_buf(),
        git_ref: git_ref.to_string(),
        root,
        ext_stats,
        overall,
        skipped_bin,
        file_count: infos.len() as u64,
    })
}

fn build_tree(infos: &[FileInfo]) -> DirNode {
    let mut top_stats: HashMap<String, DirStats> = HashMap::new();
    let mut root_stats = DirStats::default();

    for i in infos {
        root_stats.agg.add(i);
        *root_stats.ext_counts.entry(i.ext.clone()).or_insert(0) += 1;

        let top = top_level_dir(&i.path);
        let ds = top_stats.entry(top).or_insert_with(DirStats::default);
        ds.agg.add(i);
        *ds.ext_counts.entry(i.ext.clone()).or_insert(0) += 1;
    }

    let mut root = DirNode {
        name: ".".to_string(),
        full_path: "".to_string(),
        children: vec![],
        files: vec![],
        stats: root_stats,
    };

    let mut by_top: HashMap<String, Vec<&FileInfo>> = HashMap::new();
    for i in infos {
        let top = top_level_dir(&i.path);
        by_top.entry(top).or_default().push(i);
    }

    if let Some(root_files) = by_top.remove(".") {
        for i in root_files {
            root.files.push(file_row(i));
        }
        root.files.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let mut tops: Vec<String> = by_top.keys().cloned().collect();
    tops.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

    for top in tops {
        let items = by_top.get(&top).unwrap();
        let ds = top_stats.get(&top).cloned().unwrap_or_default();
        root.children.push(build_subtree(top.clone(), items, ds));
    }

    root
}

#[derive(Default)]
struct NodeBuild {
    dirs: HashMap<String, NodeBuild>,
    files: Vec<FileRow>,
}

fn build_subtree(top_name: String, items: &[&FileInfo], top_dir_stats: DirStats) -> DirNode {
    let mut node = DirNode {
        name: top_name.clone(),
        full_path: top_name.clone(),
        children: vec![],
        files: vec![],
        stats: top_dir_stats,
    };

    let mut nb = NodeBuild::default();

    for i in items {
        let rel = i
            .path
            .strip_prefix(&format!("{}/", top_name))
            .unwrap_or(&i.path);
        let parts: Vec<&str> = rel.split('/').collect();

        if parts.len() == 1 {
            nb.files.push(file_row(i));
        } else {
            let mut cur = &mut nb;
            for d in &parts[..parts.len() - 1] {
                cur = cur.dirs.entry((*d).to_string()).or_default();
            }
            cur.files.push(file_row(i));
        }
    }

    fn sort_node(n: &mut NodeBuild) {
        n.files.sort_by(|a, b| a.name.cmp(&b.name));
        for (_, child) in n.dirs.iter_mut() {
            sort_node(child);
        }
    }
    sort_node(&mut nb);

    fn to_dirnode(name: String, full_path: String, nb: NodeBuild) -> DirNode {
        let mut children: Vec<DirNode> = nb
            .dirs
            .into_iter()
            .map(|(dname, child)| {
                let child_full = if full_path.is_empty() {
                    dname.clone()
                } else {
                    format!("{}/{}", full_path, dname)
                };
                to_dirnode(dname, child_full, child)
            })
            .collect();
        children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        DirNode {
            name,
            full_path,
            children,
            files: nb.files,
            stats: DirStats::default(), // nested stats hidden
        }
    }

    node.files = nb.files;
    node.children = nb
        .dirs
        .into_iter()
        .map(|(dname, child)| {
            let full = format!("{}/{}", node.full_path, dname);
            to_dirnode(dname, full, child)
        })
        .collect();
    node.children
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    node
}
