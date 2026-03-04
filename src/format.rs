use crate::model::DirStats;

pub fn format_top_stats(ds: &DirStats, max_exts: usize) -> String {
    let total_files: u64 = ds.ext_files.values().copied().sum();

    let mut exts: Vec<&String> = ds.ext_files.keys().collect();
    exts.sort_by(|a, b| {
        let af = ds.ext_files.get(*a).copied().unwrap_or(0);
        let bf = ds.ext_files.get(*b).copied().unwrap_or(0);
        let al = ds.ext_loc.get(*a).copied().unwrap_or(0);
        let bl = ds.ext_loc.get(*b).copied().unwrap_or(0);
        bf.cmp(&af).then_with(|| bl.cmp(&al)).then_with(|| a.cmp(b))
    });

    let mut parts: Vec<String> = Vec::new();
    let mut other_files: u64 = 0;
    let mut other_loc: u64 = 0;

    for (idx, ext) in exts.iter().enumerate() {
        let files = ds.ext_files.get(*ext).copied().unwrap_or(0);
        let loc = ds.ext_loc.get(*ext).copied().unwrap_or(0);

        if idx < max_exts {
            parts.push(format!("{}:{}/{}", ext, files, loc));
        } else {
            other_files += files;
            other_loc += loc;
        }
    }

    if other_files > 0 {
        parts.push(format!("other:{}/{}", other_files, other_loc));
    }

    if parts.is_empty() {
        format!("Files:{}", total_files)
    } else {
        format!("Files:{} | {}", total_files, parts.join(", "))
    }
}

pub fn join_excludes(excludes: &[String]) -> String {
    excludes.join(", ")
}

pub fn parse_excludes(joined: &str) -> Vec<String> {
    joined
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
