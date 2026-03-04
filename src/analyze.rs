use crate::app::state::WORKTREE_REF;
use crate::git;
use crate::model::{AnalysisResult, DirNode, DirStats, FileInfo, FileRow, StatAgg};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

const BINARY_EXTS: &[&str] = &[
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".bmp",
    ".webp",
    ".ico",
    ".pdf",
    ".zip",
    ".gz",
    ".tgz",
    ".bz2",
    ".xz",
    ".7z",
    ".rar",
    ".tar",
    ".mp3",
    ".mp4",
    ".mov",
    ".avi",
    ".mkv",
    ".wav",
    ".flac",
    ".ttf",
    ".otf",
    ".woff",
    ".woff2",
    ".eot",
    ".jar",
    ".class",
    ".exe",
    ".dll",
    ".so",
    ".dylib",
    ".bin",
];

fn is_binary_ext(ext: &str) -> bool {
    BINARY_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext))
}


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


        if is_binary_ext(&ext) {
            skipped_bin += 1;
            infos.push(FileInfo {
                path: f.clone(),
                ext,
                is_text: false,
                loc: None,
            });
            continue;
        }

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
    #[derive(Default)]
    struct NodeBuild {
        dirs: HashMap<String, NodeBuild>,
        files: Vec<&'static FileInfo>,
        stats: DirStats,
    }

    fn add_to_stats(ds: &mut DirStats, i: &FileInfo) {
        *ds.ext_files.entry(i.ext.clone()).or_insert(0) += 1;
        if i.is_text {
            if let Some(loc) = i.loc {
                *ds.ext_loc.entry(i.ext.clone()).or_insert(0) += loc;
            }
        }
    }

    fn sort_node(n: &mut NodeBuild) {
        n.files.sort_by(|a, b| {
            let an = a.path.split('/').last().unwrap_or(a.path.as_str());
            let bn = b.path.split('/').last().unwrap_or(b.path.as_str());
            an.cmp(bn)
        });
        for (_, child) in n.dirs.iter_mut() {
            sort_node(child);
        }
    }

    fn build_dirnode(name: String, full_path: String, nb: &NodeBuild) -> DirNode {
        let mut files: Vec<FileRow> = nb.files.iter().map(|i| file_row(i)).collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));

        let mut keys: Vec<String> = nb.dirs.keys().cloned().collect();
        keys.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

        let mut children = Vec::with_capacity(keys.len());
        for k in keys {
            let child = nb.dirs.get(&k).unwrap();
            let child_full = if full_path.is_empty() {
                k.clone()
            } else {
                format!("{}/{}", full_path, k)
            };
            children.push(build_dirnode(k.clone(), child_full, child));
        }

        DirNode {
            name,
            full_path,
            children,
            files,
            stats: nb.stats.clone(),
        }
    }

    let mut root = NodeBuild::default();

    for i in infos {
        let i_static: &'static FileInfo = unsafe { &*(i as *const FileInfo) };

        add_to_stats(&mut root.stats, i);

        let mut cur = &mut root;
        let parts: Vec<&str> = i.path.split('/').collect();
        if parts.len() == 1 {
            cur.files.push(i_static);
            continue;
        }

        for d in &parts[..parts.len() - 1] {
            cur = cur.dirs.entry((*d).to_string()).or_default();
            add_to_stats(&mut cur.stats, i);
        }

        cur.files.push(i_static);
    }

    sort_node(&mut root);

    build_dirnode(".".to_string(), "".to_string(), &root)
}
