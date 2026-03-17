use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, DiffRow, DiffRowKind, WORKTREE_REF};
use crate::app::ui::helpers::language_hint_for_path;

use egui_extras::syntax_highlighting::highlight;

fn bg_for(kind: DiffRowKind, is_left: bool) -> Option<egui::Color32> {
    let add_bg = egui::Color32::from_rgba_unmultiplied(0, 160, 70, 14);
    let del_bg = egui::Color32::from_rgba_unmultiplied(200, 50, 50, 14);

    match kind {
        DiffRowKind::Equal => None,
        DiffRowKind::Add => {
            if is_left {
                None
            } else {
                Some(add_bg)
            }
        }
        DiffRowKind::Delete => {
            if is_left {
                Some(del_bg)
            } else {
                None
            }
        }
        DiffRowKind::Change => {
            if is_left {
                Some(del_bg)
            } else {
                Some(add_bg)
            }
        }
    }
}

fn accent_for(kind: DiffRowKind) -> Option<egui::Color32> {
    match kind {
        DiffRowKind::Equal => None,
        DiffRowKind::Add => Some(egui::Color32::from_rgb(0, 160, 70)),
        DiffRowKind::Delete => Some(egui::Color32::from_rgb(200, 50, 50)),
        DiffRowKind::Change => Some(egui::Color32::from_rgb(220, 140, 40)),
    }
}

fn monospace_char_width(ui: &egui::Ui) -> f32 {
    ui.fonts(|f| {
        let galley = f.layout_no_wrap(
            "M".to_string(),
            egui::TextStyle::Monospace.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        galley.size().x.max(1.0)
    })
}

fn fmt_ref(r: &str) -> String {
    if r == WORKTREE_REF {
        "Working tree".to_string()
    } else {
        r.to_string()
    }
}

fn is_gap_row(r: &DiffRow) -> bool {
    r.left.as_deref() == Some("…") && r.right.as_deref() == Some("…")
}

fn filtered_rows(full: &[DiffRow], only_changes: bool, ctx: usize) -> Vec<DiffRow> {
    if !only_changes {
        return full.to_vec();
    }
    if full.is_empty() {
        return vec![];
    }

    let mut keep = vec![false; full.len()];
    for (idx, r) in full.iter().enumerate() {
        if r.kind != DiffRowKind::Equal {
            let start = idx.saturating_sub(ctx);
            let end = (idx + ctx).min(full.len() - 1);
            for k in start..=end {
                keep[k] = true;
            }
        }
    }

    let mut out: Vec<DiffRow> = Vec::new();
    let mut in_gap = false;

    for (idx, r) in full.iter().enumerate() {
        if keep[idx] {
            in_gap = false;
            out.push(r.clone());
        } else if !in_gap {
            if !out.is_empty() {
                out.push(DiffRow {
                    left_no: None,
                    right_no: None,
                    left: Some("…".to_string()),
                    right: Some("…".to_string()),
                    kind: DiffRowKind::Equal,
                });
            }
            in_gap = true;
        }
    }

    let any_real = out.iter().any(|r| !is_gap_row(r));
    if any_real {
        out
    } else {
        vec![]
    }
}

fn filtered_rows_with_idx(full: &[DiffRow], only_changes: bool, ctx: usize) -> Vec<(Option<usize>, DiffRow)> {
    if !only_changes {
        return full.iter().cloned().enumerate().map(|(i, r)| (Some(i), r)).collect();
    }
    if full.is_empty() {
        return vec![];
    }

    let mut keep = vec![false; full.len()];
    for (idx, r) in full.iter().enumerate() {
        if r.kind != DiffRowKind::Equal {
            let start = idx.saturating_sub(ctx);
            let end = (idx + ctx).min(full.len() - 1);
            for k in start..=end {
                keep[k] = true;
            }
        }
    }

    let mut out: Vec<(Option<usize>, DiffRow)> = Vec::new();
    let mut in_gap = false;

    for (idx, r) in full.iter().enumerate() {
        if keep[idx] {
            in_gap = false;
            out.push((Some(idx), r.clone()));
        } else if !in_gap {
            if !out.is_empty() {
                out.push((
                    None,
                    DiffRow {
                        left_no: None,
                        right_no: None,
                        left: Some("…".to_string()),
                        right: Some("…".to_string()),
                        kind: DiffRowKind::Equal,
                    },
                ));
            }
            in_gap = true;
        }
    }

    let any_real = out.iter().any(|(_, r)| !is_gap_row(r));
    if any_real {
        out
    } else {
        vec![]
    }
}

fn expand_change_block(full: &[DiffRow], idx: usize) -> (usize, usize) {
    if full.is_empty() {
        return (0, 0);
    }
    let mut a = idx;
    let mut b = idx;

    while a > 0 && full[a - 1].kind != DiffRowKind::Equal {
        a -= 1;
    }
    while b + 1 < full.len() && full[b + 1].kind != DiffRowKind::Equal {
        b += 1;
    }
    (a, b)
}

fn counts_for_hunk(rows: &[DiffRow]) -> (usize, usize) {
    let mut old_n = 0usize;
    let mut new_n = 0usize;
    for r in rows {
        match r.kind {
            DiffRowKind::Equal => {
                if r.left_no.is_some() { old_n += 1; }
                if r.right_no.is_some() { new_n += 1; }
            }
            DiffRowKind::Add => {
                if r.right_no.is_some() { new_n += 1; }
            }
            DiffRowKind::Delete => {
                if r.left_no.is_some() { old_n += 1; }
            }
            DiffRowKind::Change => {
                if r.left_no.is_some() { old_n += 1; }
                if r.right_no.is_some() { new_n += 1; }
            }
        }
    }
    (old_n, new_n)
}

fn build_unified_patch_for_single_row(path: &str, row: &DiffRow) -> Option<String> {
    if row.kind == DiffRowKind::Equal {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!("diff --git a/{0} b/{0}\n", path));
    out.push_str(&format!("--- a/{0}\n", path));
    out.push_str(&format!("+++ b/{0}\n", path));

    match row.kind {
        DiffRowKind::Add => {
            let n = row.right_no.unwrap_or(1).max(1);
            out.push_str(&format!("@@ -{},0 +{},1 @@\n", n, n));
            out.push_str("+");
            out.push_str(row.right.as_deref().unwrap_or(""));
            out.push('\n');
        }
        DiffRowKind::Delete => {
            let n = row.left_no.unwrap_or(1).max(1);
            out.push_str(&format!("@@ -{},1 +{},0 @@\n", n, n));
            out.push_str("-");
            out.push_str(row.left.as_deref().unwrap_or(""));
            out.push('\n');
        }
        DiffRowKind::Change => {
            let old_n = row.left_no.unwrap_or(1).max(1);
            let new_n = row.right_no.unwrap_or(1).max(1);
            out.push_str(&format!("@@ -{},1 +{},1 @@\n", old_n, new_n));
            out.push_str("-");
            out.push_str(row.left.as_deref().unwrap_or(""));
            out.push('\n');
            out.push_str("+");
            out.push_str(row.right.as_deref().unwrap_or(""));
            out.push('\n');
        }
        DiffRowKind::Equal => return None,
    }

    Some(out)
}

fn build_unified_patch_for_range(path: &str, full: &[DiffRow], start: usize, end: usize, ctx: usize) -> Option<String> {
    if full.is_empty() || start > end || end >= full.len() {
        return None;
    }

    let mut left_ctx = 0usize;
    let mut a = start;
    while a > 0 && left_ctx < ctx {
        if full[a - 1].kind == DiffRowKind::Equal {
            left_ctx += 1;
            a -= 1;
        } else {
            break;
        }
    }

    let mut right_ctx = 0usize;
    let mut b = end;
    while b + 1 < full.len() && right_ctx < ctx {
        if full[b + 1].kind == DiffRowKind::Equal {
            right_ctx += 1;
            b += 1;
        } else {
            break;
        }
    }

    let window = &full[a..=b];

    let old_start = window
        .iter()
        .find_map(|r| r.left_no)
        .or_else(|| {
            window
                .iter()
                .find_map(|r| r.right_no)
                .map(|n| n.saturating_sub(1))
        })
        .unwrap_or(0);
    let new_start = window
        .iter()
        .find_map(|r| r.right_no)
        .or_else(|| {
            window
                .iter()
                .find_map(|r| r.left_no)
                .map(|n| n.saturating_sub(1))
        })
        .unwrap_or(0);

    let (old_count, new_count) = counts_for_hunk(window);

    let mut out = String::new();
    out.push_str(&format!("diff --git a/{0} b/{0}\n", path));
    out.push_str(&format!("--- a/{0}\n", path));
    out.push_str(&format!("+++ b/{0}\n", path));
    out.push_str(&format!("@@ -{},{} +{},{} @@\n", old_start, old_count, new_start, new_count));

    for r in window {
        match r.kind {
            DiffRowKind::Equal => {
                let t = r.right.as_deref().or(r.left.as_deref()).unwrap_or("");
                out.push_str(" ");
                out.push_str(t);
                out.push('\n');
            }
            DiffRowKind::Add => {
                let t = r.right.as_deref().unwrap_or("");
                out.push_str("+");
                out.push_str(t);
                out.push('\n');
            }
            DiffRowKind::Delete => {
                let t = r.left.as_deref().unwrap_or("");
                out.push_str("-");
                out.push_str(t);
                out.push('\n');
            }
            DiffRowKind::Change => {
                let lt = r.left.as_deref().unwrap_or("");
                let rt = r.right.as_deref().unwrap_or("");
                out.push_str("-");
                out.push_str(lt);
                out.push('\n');
                out.push_str("+");
                out.push_str(rt);
                out.push('\n');
            }
        }
    }

    Some(out)
}

fn render_diff_rows_body_ui(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &AppState,
    viewer_id: Option<ComponentId>,
    path: Option<&str>,
    rows: &[DiffRow],
    only_changes: bool,
    context_lines: usize,
) -> Vec<Action> {
    let mut actions = vec![];
    let _scroll_clip = ui.painter().with_clip_rect(ui.clip_rect());

    let avail_w = ui.available_width().max(40.0);
    let gutter = 12.0;
    let col_w = ((avail_w - gutter).max(40.0)) * 0.5;

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(col_w, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("OLD").strong());
            },
        );
        ui.add_space(gutter);
        ui.allocate_ui_with_layout(
            egui::vec2(col_w, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("NEW").strong());
            },
        );
    });

    ui.add_space(6.0);

    let row_h = ui
        .text_style_height(&egui::TextStyle::Monospace)
        .max(16.0)
        + 4.0;

    let rows_to_show = filtered_rows_with_idx(rows, only_changes, context_lines);

    let cw = monospace_char_width(ui);
    let ln_prefix_chars: f32 = 5.0;
    let ln_px: f32 = (ln_prefix_chars * cw).max(12.0);

    let lang = path.map(language_hint_for_path).unwrap_or("txt");

    let mono_font = egui::TextStyle::Monospace.resolve(ui.style());
    let weak = ui.visuals().weak_text_color();
    let fallback = ui.visuals().text_color();

    let row_start_y = ui.cursor().top();
    let clip = ui.clip_rect();
    let first_visible = ((clip.top() - row_start_y) / row_h).floor().max(0.0) as usize;
    let last_visible = ((clip.bottom() - row_start_y) / row_h).ceil().max(0.0) as usize;
    let visible_start = first_visible.min(rows_to_show.len());
    let visible_end = last_visible.min(rows_to_show.len());

    if visible_start > 0 {
        ui.add_space(visible_start as f32 * row_h);
    }

    for (full_idx, row) in rows_to_show[visible_start..visible_end].iter() {
        let is_gap = is_gap_row(row);

        ui.horizontal(|ui| {
            let row_clip = ui.clip_rect();

            let (rect_l, _) = ui.allocate_exact_size(egui::vec2(col_w, row_h), egui::Sense::hover());
            let clip_l = rect_l.intersect(row_clip);
            let painter_l = ui.painter().with_clip_rect(clip_l);

            if !is_gap {
                if let Some(accent) = accent_for(row.kind) {
                    let stripe = egui::Rect::from_min_max(
                        rect_l.min,
                        egui::pos2(rect_l.min.x + 3.0, rect_l.max.y),
                    );
                    painter_l.rect_filled(stripe, 0.0, accent);
                }
            }

            if !is_gap {
                if let Some(bg) = bg_for(row.kind, true) {
                    painter_l.rect_filled(rect_l, 0.0, bg);
                }
            }

            let lno = row.left_no.map(|n| format!("{:>4} ", n)).unwrap_or_else(|| "     ".to_string());
            let ltxt = row.left.clone().unwrap_or_default();

            let mut job_l = highlight(ctx, &state.theme.code_theme, ltxt.as_str(), lang);
            job_l.wrap.max_width = 10_000.0;
            let galley_l = ui.fonts(|f| f.layout_job(job_l));

            let y = rect_l.min.y + 2.0;
            painter_l.text(
                egui::pos2(rect_l.min.x + 6.0, y),
                egui::Align2::LEFT_TOP,
                lno,
                mono_font.clone(),
                weak,
            );

            if is_gap {
                painter_l.text(
                    egui::pos2(rect_l.min.x + 6.0 + ln_px, y),
                    egui::Align2::LEFT_TOP,
                    ltxt,
                    mono_font.clone(),
                    weak,
                );
            } else {
                painter_l.galley(
                    egui::pos2(rect_l.min.x + 6.0 + ln_px, y),
                    galley_l.clone(),
                    fallback,
                );
            }

            let can_revert = viewer_id.is_some() && path.is_some() && full_idx.is_some() && row.kind != DiffRowKind::Equal;
            if can_revert {
                let icon_w = gutter.max(18.0);
                let center_x = rect_l.max.x + (gutter * 0.5);
                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(center_x, rect_l.center().y),
                    egui::vec2(icon_w, row_h),
                );

                let resp = ui
                    .allocate_rect(icon_rect, egui::Sense::click())
                    .on_hover_text("Click: revert contiguous change block");

                let hovered = resp.hovered();
                let col = if hovered {
                    ui.visuals().text_color()
                } else {
                    ui.visuals().weak_text_color()
                };

                let font = egui::FontId::monospace((row_h * 0.85).clamp(14.0, 22.0));
                ui.painter().text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "↶",
                    font,
                    col,
                );

                if resp.clicked() {
                    let viewer_id = viewer_id.unwrap();
                    let path = path.unwrap();
                    let idx0 = full_idx.unwrap();

                    let (a, b) = expand_change_block(rows, idx0);
                    let patch = build_unified_patch_for_range(path, rows, a, b, 3);

                    if let Some(patch) = patch {
                        actions.push(Action::DiffViewerRevertPatch { viewer_id, patch });
                        actions.push(Action::RefreshDiffViewer { viewer_id });
                        for sc_id in state.source_controls.keys().copied() {
                            actions.push(Action::RefreshSourceControl { sc_id });
                        }
                    }
                }
            }

            ui.add_space(gutter);

            let (rect_r, _) = ui.allocate_exact_size(egui::vec2(col_w, row_h), egui::Sense::hover());
            let clip_r = rect_r.intersect(row_clip);
            let painter_r = ui.painter().with_clip_rect(clip_r);

            if !is_gap {
                if let Some(accent) = accent_for(row.kind) {
                    let stripe = egui::Rect::from_min_max(
                        rect_r.min,
                        egui::pos2(rect_r.min.x + 3.0, rect_r.max.y),
                    );
                    painter_r.rect_filled(stripe, 0.0, accent);
                }
            }

            if !is_gap {
                if let Some(bg) = bg_for(row.kind, false) {
                    painter_r.rect_filled(rect_r, 0.0, bg);
                }
            }

            let rno = row.right_no.map(|n| format!("{:>4} ", n)).unwrap_or_else(|| "     ".to_string());
            let rtxt = row.right.clone().unwrap_or_default();

            let mut job_r = highlight(ctx, &state.theme.code_theme, rtxt.as_str(), lang);
            job_r.wrap.max_width = 10_000.0;
            let galley_r = ui.fonts(|f| f.layout_job(job_r));

            painter_r.text(
                egui::pos2(rect_r.min.x + 6.0, y),
                egui::Align2::LEFT_TOP,
                rno,
                mono_font.clone(),
                weak,
            );

            if is_gap {
                painter_r.text(
                    egui::pos2(rect_r.min.x + 6.0 + ln_px, y),
                    egui::Align2::LEFT_TOP,
                    rtxt,
                    mono_font.clone(),
                    weak,
                );
            } else {
                painter_r.galley(
                    egui::pos2(rect_r.min.x + 6.0 + ln_px, y),
                    galley_r,
                    fallback,
                );
            }
        });
    }

    if visible_end < rows_to_show.len() {
        ui.add_space((rows_to_show.len() - visible_end) as f32 * row_h);
    }

    actions
}

pub(crate) fn render_diff_rows_ui(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &AppState,
    id_source: impl std::hash::Hash,
    path: Option<&str>,
    rows: &[DiffRow],
    only_changes: bool,
    context_lines: usize,
) {
    egui::ScrollArea::both()
        .id_source(id_source)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let _ = render_diff_rows_body_ui(ctx, ui, state, None, path, rows, only_changes, context_lines);
        });
}

pub(crate) fn render_compact_diff_rows_ui(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &AppState,
    id_source: impl std::hash::Hash,
    path: Option<&str>,
    rows: &[DiffRow],
    only_changes: bool,
    context_lines: usize,
    max_height: f32,
) {
    egui::ScrollArea::both()
        .id_source(id_source)
        .auto_shrink([false, false])
        .max_height(max_height)
        .show(ui, |ui| {
            let _ = render_diff_rows_body_ui(
                ctx,
                ui,
                state,
                None,
                path,
                rows,
                only_changes,
                context_lines,
            );
        });
}

fn diff_row_stats(rows: &[DiffRow]) -> (usize, usize) {
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for row in rows {
        match row.kind {
            DiffRowKind::Add => additions += 1,
            DiffRowKind::Delete => deletions += 1,
            DiffRowKind::Change => {
                additions += 1;
                deletions += 1;
            }
            DiffRowKind::Equal => {}
        }
    }

    (additions, deletions)
}

fn render_grouped_diff_header_ui(
    ui: &mut egui::Ui,
    path: &str,
    from_ref: &str,
    to_ref: &str,
    additions: usize,
    deletions: usize,
    open: &mut bool,
) -> egui::Response {
    let frame = egui::Frame::none()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .inner_margin(egui::Margin::symmetric(8.0, 6.0));

    let inner = frame.show(ui, |ui| {
        ui.set_min_height(30.0);
        ui.horizontal(|ui| {
            let toggle = if *open { "▼" } else { "▶" };
            if ui.add_sized([24.0, 22.0], egui::Button::new(toggle)).clicked() {
                *open = !*open;
            }

            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(path)
                            .monospace()
                            .strong(),
                    )
                    .frame(false),
                )
                .clicked()
            {
                *open = !*open;
            }

            ui.separator();
            ui.label(egui::RichText::new(format!("{} → {}", fmt_ref(from_ref), fmt_ref(to_ref))).strong());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("-{}", deletions)).strong());
                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("+{}", additions)).strong());
            });
        });
    });

    inner.response
}

fn render_grouped_diff_sticky_header_overlay(
    ui: &egui::Ui,
    viewer_id: ComponentId,
    rect: egui::Rect,
    path: &str,
    from_ref: &str,
    to_ref: &str,
    additions: usize,
    deletions: usize,
    open: bool,
) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(("diff_viewer_group_sticky_header", viewer_id)),
    ));

    let visuals = ui.visuals();
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    painter.rect_stroke(rect, 0.0, visuals.widgets.active.bg_stroke);

    let text_color = visuals.text_color();
    let weak = visuals.weak_text_color();
    let strong_font = egui::TextStyle::Button.resolve(ui.style());
    let mono_font = egui::TextStyle::Monospace.resolve(ui.style());

    let mut x = rect.left() + 8.0;
    let y = rect.center().y;

    painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        if open { "▼" } else { "▶" },
        strong_font.clone(),
        text_color,
    );
    x += 18.0;

    painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        path,
        mono_font,
        text_color,
    );

    let right_text = format!("+{}   -{}   {} → {}", additions, deletions, fmt_ref(from_ref), fmt_ref(to_ref));
    painter.text(
        egui::pos2(rect.right() - 8.0, y),
        egui::Align2::RIGHT_CENTER,
        right_text,
        strong_font,
        weak,
    );
}

pub fn diff_viewer_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewer_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    {
        let Some(v) = state.diff_viewers.get_mut(&viewer_id) else {
            ui.label("Diff Viewer state missing (try resetting layout/workspace).");
            return actions;
        };

        ui.horizontal(|ui| {
            let title = v
                .aggregate_title
                .clone()
                .or_else(|| v.path.clone())
                .unwrap_or_else(|| "(no file selected)".to_string());

            ui.heading(title);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh").clicked() {
                    actions.push(Action::RefreshDiffViewer { viewer_id });
                }

                ui.separator();

                ui.checkbox(&mut v.only_changes, "Only show changes");
                ui.add_space(6.0);

                let mut ctx_lines_i = v.context_lines as i32;
                ui.add(
                    egui::DragValue::new(&mut ctx_lines_i)
                        .clamp_range(0..=50)
                        .prefix("ctx: "),
                );
                v.context_lines = (ctx_lines_i.max(0)) as usize;

                ui.separator();
                ui.label(format!("{}  →  {}", fmt_ref(&v.from_ref), fmt_ref(&v.to_ref)));
            });
        });
    }

    let Some(v) = state.diff_viewers.get(&viewer_id) else {
        return actions;
    };

    if let Some(err) = &v.last_error {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        ui.separator();
    }

    ui.add_space(6.0);

    if v.aggregate_title.is_some() {
        ui.horizontal(|ui| {
            if ui.button("Expand All").clicked() {
                for section in v.file_sections.iter() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new(("diff_viewer_aggregate_open", viewer_id, &section.path)),
                            true,
                        )
                    });
                }
            }
            if ui.button("Collapse All").clicked() {
                for section in v.file_sections.iter() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new(("diff_viewer_aggregate_open", viewer_id, &section.path)),
                            false,
                        )
                    });
                }
            }
            ui.separator();
            if v.loading {
                ui.label(format!("{} / {} files", v.loaded_files, v.total_files));
            } else {
                ui.label(format!("{} files", v.file_sections.len()));
            }
        });

        if v.loading {
            ui.add_space(4.0);
            let progress = if v.total_files == 0 {
                0.0
            } else {
                v.loaded_files as f32 / v.total_files as f32
            };
            ui.add(egui::ProgressBar::new(progress).show_percentage());
        }

        ui.add_space(6.0);

        egui::ScrollArea::both()
            .id_source(("diff_viewer_aggregate_scroll", viewer_id))
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, _viewport| {
                if v.file_sections.is_empty() {
                    if v.loading {
                        ui.label("Loading diff...");
                    } else {
                        ui.label("(no files)");
                    }
                    return;
                }

                let visible_top = ui.clip_rect().top();
                let mut active_header: Option<(egui::Rect, usize, bool, usize, usize)> = None;
                let mut next_header_top: Option<f32> = None;

                for (idx, section) in v.file_sections.iter().enumerate() {
                    let open_id = egui::Id::new(("diff_viewer_aggregate_open", viewer_id, &section.path));
                    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(open_id).unwrap_or(true));
                    let (additions, deletions) = diff_row_stats(&section.rows);

                    let header_resp = render_grouped_diff_header_ui(
                        ui,
                        &section.path,
                        &section.from_ref,
                        &section.to_ref,
                        additions,
                        deletions,
                        &mut open,
                    );

                    let header_rect = header_resp.rect;
                    if header_rect.top() <= visible_top {
                        active_header = Some((header_rect, idx, open, additions, deletions));
                        next_header_top = None;
                    } else if active_header.is_some() && next_header_top.is_none() {
                        next_header_top = Some(header_rect.top());
                    } else if active_header.is_none() {
                        active_header = Some((header_rect, idx, open, additions, deletions));
                        next_header_top = None;
                    }

                    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));

                    if open {
                        ui.add_space(4.0);
                        actions.extend(render_diff_rows_body_ui(
                            ctx,
                            ui,
                            state,
                            Some(viewer_id),
                            Some(&section.path),
                            &section.rows,
                            v.only_changes,
                            v.context_lines,
                        ));
                    }

                    if idx + 1 < v.file_sections.len() {
                        ui.add_space(8.0);
                    }
                }

                if let Some((base_rect, section_idx, open, additions, deletions)) = active_header {
                    let section = &v.file_sections[section_idx];
                    let mut sticky_rect = egui::Rect::from_min_size(
                        egui::pos2(base_rect.left(), visible_top),
                        base_rect.size(),
                    );

                    if let Some(next_top) = next_header_top {
                        let max_top = next_top - sticky_rect.height();
                        if sticky_rect.top() > max_top {
                            sticky_rect = sticky_rect.translate(egui::vec2(0.0, max_top - sticky_rect.top()));
                        }
                    }

                    render_grouped_diff_sticky_header_overlay(
                        ui,
                        viewer_id,
                        sticky_rect,
                        &section.path,
                        &section.from_ref,
                        &section.to_ref,
                        additions,
                        deletions,
                        open,
                    );
                }
            });
    } else {
        egui::ScrollArea::both()
            .id_source(("diff_viewer_scroll", viewer_id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                actions.extend(render_diff_rows_body_ui(
                    ctx,
                    ui,
                    state,
                    Some(viewer_id),
                    v.path.as_deref(),
                    &v.rows,
                    v.only_changes,
                    v.context_lines,
                ));
            });
    }

    actions
}
