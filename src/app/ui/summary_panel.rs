use eframe::egui;
use crate::model::AnalysisResult;

pub fn summary_panel(ui: &mut egui::Ui, res: &AnalysisResult) {
    ui.heading("Totals by extension");
    ui.add_space(6.0);

    ui.monospace(format!(
        "{:>10}  {:>6}  {:>6}  {:>8}  {:>6}  {:>6}  ext",
        "LOC", "files", "text", "avg", "min", "max"
    ));
    ui.separator();

    for (ext, agg) in &res.ext_stats {
        let min_s = agg.loc_min.map(|v| v.to_string()).unwrap_or("-".into());
        let max_s = agg.loc_max.map(|v| v.to_string()).unwrap_or("-".into());
        ui.monospace(format!(
            "{:>10}  {:>6}  {:>6}  {:>8.1}  {:>6}  {:>6}  {}",
            agg.loc_total, agg.files_all, agg.files_text, agg.avg(), min_s, max_s, ext
        ));
    }

    ui.separator();
    let o = &res.overall;
    let min_s = o.loc_min.map(|v| v.to_string()).unwrap_or("-".into());
    let max_s = o.loc_max.map(|v| v.to_string()).unwrap_or("-".into());
    ui.monospace(format!(
        "{:>10}  {:>6}  {:>6}  {:>8.1}  {:>6}  {:>6}  (all)",
        o.loc_total, o.files_all, o.files_text, o.avg(), min_s, max_s
    ));

    ui.add_space(8.0);
    ui.label(format!("Files: {}  (bin/unreadable: {})", res.file_count, res.skipped_bin));
}
