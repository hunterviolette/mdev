use crate::app::state::WORKTREE_REF;
use crate::git;
use crate::model::{AnalysisResult, DirNode, DirStats, FileInfo, FileRow, StatAgg};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

fn decode_lines(blob: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(blob)
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn is_binary(blob: &[u8]) -> bool {
    blob.iter().any(|&b| b == 0)
}

fn count_loc(blob: &[u8]) -> u64 {
    let s = String::from_utf8_lossy(blob);
    s.lines().count() as u64
}

fn ext_of(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if ext.is_empty() {
        "(no ext)".to_string()
    } else {
        format!(".{}", ext.to_lowercase())
    }
}

fn top_level_dir(path: &str) -> String {
    match path.split_once('/') {
        Some((top, _)) => top.to_string(),
        None => ".".to_string(),
    }
}

fn file_row(i: &FileInfo) -> FileRow {
    let name = i.path.split('/').last().unwrap_or(&i.path).to_string();
    let loc_display = if i.is_text {
        i.loc.map(|v| v.to_string()).unwrap_or_else(|| "(bin)".to_string())
    } else {
        "(bin)".to_string()
    };
    FileRow {
        name,
        full_path: i.path.clone(),
        loc_display,
        ext: i.ext.clone(),
    }
}

pub fn analyze_repo(
    repo: &Path,
    git_ref: &str,
    exclude: &[Regex],
    _max_exts_shown: usize,
) -> Result<AnalysisResult> {
    // WORKTREE: include tracked + untracked (so file tree matches disk and context export can include new files)
    let mut files: Vec<String> = if git_ref == WORKTREE_REF {
        git::list_worktree_files(repo)?
    } else {
        let ls = git::run_git(repo, &["ls-tree", "-r", "--name-only", git_ref])?;
        decode_lines(&ls)
    };

    files = files
        .into_iter()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .filter(|f| !exclude.iter().any(|rx| rx.is_match(f)))
        .collect();

    files.sort();
    files.dedup();

    let mut infos: Vec<FileInfo> = Vec::with_capacity(files.len());
    let mut skipped_bin = 0u64;

    for f in &files {
        let ext = ext_of(f);

        let blob: Vec<u8> = if git_ref == WORKTREE_REF {
            match git::read_worktree_file(repo, f) {
                Ok(b) => b,
                Err(_) => {
                    skipped_bin += 1;
                    infos.push(FileInfo {
                        path: f.clone(),
                        ext,
                        is_text: false,
                        loc: None,
                    });
                    continue;
                }
            }
        } else {
            let spec = format!("{}:{}", git_ref, f);
            let (code, b, _stderr) = git::run_git_allow_fail(repo, &["show", &spec])?;
            if code != 0 {
                skipped_bin += 1;
                infos.push(FileInfo {
                    path: f.clone(),
                    ext,
                    is_text: false,
                    loc: None,
                });
                continue;
            }
            b
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

    fn build_children(parent_full: &str, map: &HashMap<String, NodeBuild>) -> Vec<DirNode> {
        let mut keys: Vec<String> = map.keys().cloned().collect();
        keys.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

        let mut out = vec![];
        for k in keys {
            let child = map.get(&k).unwrap();
            let full = if parent_full.is_empty() {
                k.clone()
            } else {
                format!("{}/{}", parent_full, k)
            };

            let dn = DirNode {
                name: k.clone(),
                full_path: full.clone(),
                children: build_children(&full, &child.dirs),
                files: child.files.clone(),
                stats: DirStats::default(), // only root + top-level stats are meaningful currently
            };

            out.push(dn);
        }
        out
    }

    node.files = nb.files;
    node.children = build_children(&node.full_path, &nb.dirs);
    node
}
