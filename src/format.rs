use crate::model::DirStats;
use std::collections::HashMap;

pub fn format_top_stats(ds: &DirStats, max_exts: usize) -> String {
    let min_s = ds.agg.loc_min.map(|v| v.to_string()).unwrap_or("-".into());
    let max_s = ds.agg.loc_max.map(|v| v.to_string()).unwrap_or("-".into());

    let mut items: Vec<(&String, &u64)> = ds.ext_counts.iter().collect();
    items.sort_by(|(e1, c1), (e2, c2)| c2.cmp(c1).then_with(|| e1.cmp(e2)));

    let mut parts = Vec::new();
    let mut other_sum = 0u64;
    for (idx, (ext, cnt)) in items.iter().enumerate() {
        if idx < max_exts {
            parts.push(format!("{}:{}", ext, cnt));
        } else {
            other_sum += **cnt;
        }
    }
    if other_sum > 0 {
        parts.push(format!("other:{}", other_sum));
    }

    format!(
        "files:{} text:{} avg:{:.1} min:{} max:{} | {}",
        ds.agg.files_all,
        ds.agg.files_text,
        ds.agg.avg(),
        min_s,
        max_s,
        parts.join(", ")
    )
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

pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

pub fn format_ext_counts(ext_counts: &HashMap<String, u64>, max_exts: usize) -> String {
    let mut items: Vec<(&String, &u64)> = ext_counts.iter().collect();
    items.sort_by(|(e1, c1), (e2, c2)| c2.cmp(c1).then_with(|| e1.cmp(e2)));

    let mut parts = Vec::new();
    let mut other_sum = 0u64;

    for (idx, (ext, cnt)) in items.iter().enumerate() {
        if idx < max_exts {
            parts.push(format!("{}:{}", ext, cnt));
        } else {
            other_sum += **cnt;
        }
    }
    if other_sum > 0 {
        parts.push(format!("other:{}", other_sum));
    }
    parts.join(", ")
}
