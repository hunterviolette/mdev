use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct FileInfo {
    pub path: String,
    pub ext: String,
    pub is_text: bool,
    pub loc: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct StatAgg {
    pub files_all: u64,
    pub files_text: u64,
    pub loc_total: u64,
    pub loc_min: Option<u64>,
    pub loc_max: Option<u64>,
}

impl StatAgg {
    pub fn add(&mut self, info: &FileInfo) {
        self.files_all += 1;
        if info.is_text {
            if let Some(loc) = info.loc {
                self.files_text += 1;
                self.loc_total += loc;
                self.loc_min = Some(self.loc_min.map_or(loc, |m| m.min(loc)));
                self.loc_max = Some(self.loc_max.map_or(loc, |m| m.max(loc)));
            }
        }
    }

    pub fn avg(&self) -> f64 {
        if self.files_text == 0 {
            0.0
        } else {
            self.loc_total as f64 / self.files_text as f64
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DirStats {
    pub agg: StatAgg,
    // counts ALL files (text+bin) by extension
    pub ext_counts: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
pub struct FileRow {
    pub name: String,
    pub full_path: String,   // repo-relative
    pub loc_display: String, // "123" or "(bin)"
    pub ext: String,
}

#[derive(Clone, Debug)]
pub struct DirNode {
    pub name: String,
    pub full_path: String, // repo-relative directory path ("" for root)
    pub children: Vec<DirNode>,
    pub files: Vec<FileRow>,
    // only meaningful for root + top-level dirs
    pub stats: DirStats,
}

#[derive(Clone, Debug)]
pub struct AnalysisResult {
    pub repo: std::path::PathBuf,
    pub git_ref: String,
    pub root: DirNode,
    pub ext_stats: Vec<(String, StatAgg)>, // sorted by LOC desc
    pub overall: StatAgg,
    pub skipped_bin: u64,
    pub file_count: u64,
}

#[derive(Clone, Debug)]
pub struct CommitEntry {
    pub hash: String,
    pub date: String,
    pub summary: String,
}
