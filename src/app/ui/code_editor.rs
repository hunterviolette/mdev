// src/app/ui/code_editor.rs
use std::collections::HashMap;

use eframe::egui;
use egui_extras::syntax_highlighting::highlight;

use crate::app::theme;
use crate::app::ui::helpers::language_hint_for_path;

/// Persistent editor state (cursor, selection, caches).
#[derive(Clone, Debug)]
pub struct CodeEditorState {
    pub cursor_cc: usize,
    pub selection_anchor_cc: Option<usize>,
    pub buffer_version: u64,
    pub line_cache: HashMap<usize, (u64, u32, egui::text::LayoutJob)>,
    pub has_focus: bool,

    // Undo/Redo
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    frame_snapshot_taken: bool,
    undo_capacity: usize,

    // Find/Replace
    pub find_open: bool,
    pub find_query: String,
    pub replace_query: String,
    pub find_match_index: usize,
    pub find_match_case: bool, // "Aa" (exact case)
    pub find_whole_word: bool, // "W" (whole-word / exact match)

    //  track the "current" find match (Prev/Next) so we can color it differently
    pub active_find_match_cc: Option<(usize, usize)>,

    // Mouse selection
    mouse_selecting: bool,
    mouse_anchor_cc: Option<usize>, // persistent drag anchor across frames

    // Scroll state (for PgUp/PgDn scrolling)
    scroll_offset: egui::Vec2,

    // Scroll-to support (e.g. Find next/prev)
    pending_scroll_to_cc: Option<usize>,
}

#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    cursor_cc: usize,
    selection_anchor_cc: Option<usize>,
}

impl Default for CodeEditorState {
    fn default() -> Self {
        Self {
            cursor_cc: 0,
            selection_anchor_cc: None,
            buffer_version: 0,
            line_cache: HashMap::new(),
            has_focus: false,

            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            frame_snapshot_taken: false,
            undo_capacity: 50,

            find_open: false,
            find_query: String::new(),
            replace_query: String::new(),
            find_match_index: 0,
            find_match_case: false,
            find_whole_word: false,

            active_find_match_cc: None,

            mouse_selecting: false,
            mouse_anchor_cc: None,

            scroll_offset: egui::Vec2::ZERO,
            pending_scroll_to_cc: None,
        }
    }
}

fn cc_len(s: &str) -> usize {
    s.chars().count()
}

fn clamp_cc(s: &str, cc: usize) -> usize {
    cc.min(cc_len(s))
}

/// Convert char-index to byte-index (O(n), used only on edits/selection ops).
fn cc_to_bc(s: &str, cc: usize) -> usize {
    if cc == 0 {
        return 0;
    }
    let mut cur = 0usize;
    for (bi, _ch) in s.char_indices() {
        if cur == cc {
            return bi;
        }
        cur += 1;
    }
    s.len()
}

/// Convert byte-index to char-index (O(n)).
fn bc_to_cc(s: &str, bc: usize) -> usize {
    let mut cc = 0usize;
    for (bi, _ch) in s.char_indices() {
        if bi >= bc {
            break;
        }
        cc += 1;
    }
    cc
}

/// Selection range in cc (if any).
fn selection_range(st: &CodeEditorState) -> Option<(usize, usize)> {
    let a = st.selection_anchor_cc?;
    let b = st.cursor_cc;
    if a == b {
        None
    } else if a < b {
        Some((a, b))
    } else {
        Some((b, a))
    }
}

fn clear_selection(st: &mut CodeEditorState) {
    st.selection_anchor_cc = None;
}

fn set_selection(st: &mut CodeEditorState, a: usize, b: usize) {
    st.selection_anchor_cc = Some(a);
    st.cursor_cc = b;
}

fn delete_selection(text: &mut String, st: &mut CodeEditorState) -> bool {
    let Some((a, b)) = selection_range(st) else { return false; };
    let a_bc = cc_to_bc(text, a);
    let b_bc = cc_to_bc(text, b);
    text.replace_range(a_bc..b_bc, "");
    st.cursor_cc = a;
    st.selection_anchor_cc = None;
    true
}

fn insert_text(text: &mut String, st: &mut CodeEditorState, ins: &str) -> bool {
    let _ = delete_selection(text, st);
    let cur = clamp_cc(text, st.cursor_cc);
    let bi = cc_to_bc(text, cur);
    text.insert_str(bi, ins);
    st.cursor_cc = cur + ins.chars().count();
    true
}

fn delete_prev_char(text: &mut String, st: &mut CodeEditorState) -> bool {
    if delete_selection(text, st) {
        return true;
    }
    let cur = clamp_cc(text, st.cursor_cc);
    if cur == 0 {
        return false;
    }
    let a_cc = cur - 1;
    let a_bc = cc_to_bc(text, a_cc);
    let b_bc = cc_to_bc(text, cur);
    text.replace_range(a_bc..b_bc, "");
    st.cursor_cc = a_cc;
    true
}

fn delete_next_char(text: &mut String, st: &mut CodeEditorState) -> bool {
    if delete_selection(text, st) {
        return true;
    }
    let cur = clamp_cc(text, st.cursor_cc);
    let len = cc_len(text);
    if cur >= len {
        return false;
    }
    let a_bc = cc_to_bc(text, cur);
    let b_bc = cc_to_bc(text, cur + 1);
    text.replace_range(a_bc..b_bc, "");
    true
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn delete_prev_word(text: &mut String, st: &mut CodeEditorState) -> bool {
    if delete_selection(text, st) {
        return true;
    }

    let cur = clamp_cc(text, st.cursor_cc);
    if cur == 0 {
        return false;
    }

    let chars: Vec<char> = text.chars().collect();
    let mut i = cur;

    while i > 0 && !is_word_char(chars[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1]) {
        i -= 1;
    }

    let a_cc = i;
    let a_bc = cc_to_bc(text, a_cc);
    let b_bc = cc_to_bc(text, cur);
    text.replace_range(a_bc..b_bc, "");
    st.cursor_cc = a_cc;
    true
}

fn move_cursor_left(text: &str, st: &mut CodeEditorState, selecting: bool) {
    let cur = clamp_cc(text, st.cursor_cc);
    if !selecting {
        clear_selection(st);
    } else if st.selection_anchor_cc.is_none() {
        st.selection_anchor_cc = Some(cur);
    }
    st.cursor_cc = cur.saturating_sub(1);
}

fn move_cursor_right(text: &str, st: &mut CodeEditorState, selecting: bool) {
    let cur = clamp_cc(text, st.cursor_cc);
    let len = cc_len(text);
    if !selecting {
        clear_selection(st);
    } else if st.selection_anchor_cc.is_none() {
        st.selection_anchor_cc = Some(cur);
    }
    st.cursor_cc = (cur + 1).min(len);
}

fn move_word_left(text: &str, st: &mut CodeEditorState, selecting: bool) {
    let cur = clamp_cc(text, st.cursor_cc);
    if cur == 0 {
        if !selecting {
            clear_selection(st);
        }
        return;
    }

    let chars: Vec<char> = text.chars().collect();
    let mut i = cur;

    while i > 0 && !is_word_char(chars[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1]) {
        i -= 1;
    }

    if !selecting {
        clear_selection(st);
    } else if st.selection_anchor_cc.is_none() {
        st.selection_anchor_cc = Some(cur);
    }
    st.cursor_cc = i;
}

fn move_word_right(text: &str, st: &mut CodeEditorState, selecting: bool) {
    let cur = clamp_cc(text, st.cursor_cc);
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = cur;

    while i < len && is_word_char(chars[i]) {
        i += 1;
    }
    while i < len && !is_word_char(chars[i]) {
        i += 1;
    }

    if !selecting {
        clear_selection(st);
    } else if st.selection_anchor_cc.is_none() {
        st.selection_anchor_cc = Some(cur);
    }
    st.cursor_cc = i.min(len);
}

/// Compute (line, col) from cc, clamped safely to the current text.
fn cc_to_lc_safe(text: &str, cc: usize) -> (usize, usize) {
    let cc = clamp_cc(text, cc);
    let mut cur_cc = 0usize;
    let mut line = 0usize;
    let mut col = 0usize;
    for ch in text.chars() {
        if cur_cc == cc {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        cur_cc += 1;
    }
    (line, col)
}

/// Numeric line metrics (no borrowing `text` slices).
fn compute_line_metrics(text: &str) -> (Vec<usize>, Vec<usize>) {
    let mut starts: Vec<usize> = vec![0];
    let mut lens: Vec<usize> = Vec::new();

    let mut cur_line_len = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            lens.push(cur_line_len);
            cur_line_len = 0;

            let prev_start = *starts.last().unwrap_or(&0);
            let prev_len = *lens.last().unwrap_or(&0);
            starts.push(prev_start + prev_len + 1);
        } else {
            cur_line_len += 1;
        }
    }
    lens.push(cur_line_len);

    while starts.len() < lens.len() {
        starts.push(*starts.last().unwrap_or(&0));
    }
    if starts.is_empty() {
        starts.push(0);
    }
    if lens.is_empty() {
        lens.push(0);
    }

    (starts, lens)
}

fn lc_to_cc_metrics(starts: &[usize], lens: &[usize], line: usize, col: usize) -> usize {
    if starts.is_empty() || lens.is_empty() {
        return 0;
    }
    let line = line.min(lens.len().saturating_sub(1));
    let col = col.min(lens[line]);
    starts[line] + col
}

// --- Scroll-to helpers (for Find next/prev, etc.) ---

fn request_scroll_to_cc(st: &mut CodeEditorState, cc: usize) {
    st.pending_scroll_to_cc = Some(cc);
}

/// Ensure a given cc is visible by adjusting vertical scroll offset.
fn apply_scroll_to_cc_if_needed(
    text: &str,
    st: &mut CodeEditorState,
    row_height: f32,
    viewport_h: f32,
) -> bool {
    let Some(cc) = st.pending_scroll_to_cc.take() else {
        return false;
    };

    if viewport_h <= 1.0 || row_height <= 1.0 {
        return false;
    }

    let (line, _col) = cc_to_lc_safe(text, cc);
    let y = (line as f32) * row_height;

    let margin = (row_height * 2.0).min(viewport_h * 0.25);

    let top = st.scroll_offset.y;
    let bottom = st.scroll_offset.y + viewport_h;

    let desired_top = (y - margin).max(0.0);
    let desired_bottom = y + row_height + margin;

    if y < top + margin {
        st.scroll_offset.y = desired_top;
        return true;
    }

    if desired_bottom > bottom {
        st.scroll_offset.y = (desired_bottom - viewport_h).max(0.0);
        return true;
    }

    false
}

fn snapshot(text: &str, st: &CodeEditorState) -> Snapshot {
    Snapshot {
        text: text.to_string(),
        cursor_cc: st.cursor_cc,
        selection_anchor_cc: st.selection_anchor_cc,
    }
}

impl CodeEditorState {
    fn begin_frame(&mut self) {
        self.frame_snapshot_taken = false;
    }

    fn take_undo_snapshot_if_needed(&mut self, text: &str) {
        if self.frame_snapshot_taken {
            return;
        }
        self.frame_snapshot_taken = true;

        self.undo_stack.push(snapshot(text, self));
        if self.undo_stack.len() > self.undo_capacity {
            let overflow = self.undo_stack.len() - self.undo_capacity;
            self.undo_stack.drain(0..overflow);
        }
        self.redo_stack.clear();
    }

    fn apply_snapshot(&mut self, text: &mut String, snap: Snapshot) {
        *text = snap.text;
        self.cursor_cc = snap.cursor_cc;
        self.selection_anchor_cc = snap.selection_anchor_cc;
        self.buffer_version = self.buffer_version.wrapping_add(1);
        self.line_cache.clear();
    }

    fn undo(&mut self, text: &mut String) {
        let Some(prev) = self.undo_stack.pop() else { return; };
        self.redo_stack.push(snapshot(text, self));
        self.apply_snapshot(text, prev);
    }

    fn redo(&mut self, text: &mut String) {
        let Some(next) = self.redo_stack.pop() else { return; };
        self.undo_stack.push(snapshot(text, self));
        if self.undo_stack.len() > self.undo_capacity {
            let overflow = self.undo_stack.len() - self.undo_capacity;
            self.undo_stack.drain(0..overflow);
        }
        self.apply_snapshot(text, next);
    }
}

// --- Find helpers ---

fn ascii_eq_ignore_case(a: u8, b: u8) -> bool {
    if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
        a.to_ascii_lowercase() == b.to_ascii_lowercase()
    } else {
        a == b
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_whole_word_boundary(tb: &[u8], start: usize, end: usize) -> bool {
    let left_ok = if start == 0 { true } else { !is_word_byte(tb[start - 1]) };
    let right_ok = if end >= tb.len() { true } else { !is_word_byte(tb[end]) };
    left_ok && right_ok
}

fn find_all_matches(
    text: &str,
    query: &str,
    match_case: bool,
    whole_word: bool,
) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return vec![];
    }

    let tb = text.as_bytes();
    let qb = query.as_bytes();

    if qb.is_empty() || qb.len() > tb.len() {
        return vec![];
    }

    let mut out = Vec::new();
    let mut i = 0usize;

    while i + qb.len() <= tb.len() {
        let mut ok = true;
        for j in 0..qb.len() {
            let a = tb[i + j];
            let b = qb[j];
            if match_case {
                if a != b {
                    ok = false;
                    break;
                }
            } else if !ascii_eq_ignore_case(a, b) {
                ok = false;
                break;
            }
        }

        if ok {
            let end = i + qb.len();
            if !whole_word || is_whole_word_boundary(tb, i, end) {
                out.push((i, end));
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }

    out
}

fn find_line_matches_cols(
    line: &str,
    query: &str,
    match_case: bool,
    whole_word: bool,
) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return vec![];
    }

    let lb = line.as_bytes();
    let qb = query.as_bytes();
    if qb.is_empty() || qb.len() > lb.len() {
        return vec![];
    }

    let mut out = Vec::new();
    let mut i = 0usize;

    while i + qb.len() <= lb.len() {
        let mut ok = true;
        for j in 0..qb.len() {
            let a = lb[i + j];
            let b = qb[j];
            if match_case {
                if a != b {
                    ok = false;
                    break;
                }
            } else if !ascii_eq_ignore_case(a, b) {
                ok = false;
                break;
            }
        }

        if ok {
            let end = i + qb.len();
            if !whole_word || is_whole_word_boundary(lb, i, end) {
                let a_col = bc_to_cc(line, i);
                let b_col = bc_to_cc(line, end);
                if b_col > a_col {
                    out.push((a_col, b_col));
                }
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }

    out
}

//  now records which match is the "active" find match
fn select_match(text: &str, st: &mut CodeEditorState, a_bc: usize, b_bc: usize) {
    let a_cc = bc_to_cc(text, a_bc);
    let b_cc = bc_to_cc(text, b_bc);
    set_selection(st, a_cc, b_cc);
    st.active_find_match_cc = Some((a_cc.min(b_cc), a_cc.max(b_cc)));
}

// --- Monospace measuring (fast + stable) ---

fn monospace_char_width(ui: &egui::Ui) -> f32 {
    ui.fonts(|f| {
        f.layout_no_wrap(
            "M".to_string(),
            egui::FontId::monospace(12.0),
            ui.visuals().text_color(),
        )
        .size()
        .x
    })
    .max(1.0)
}

fn click_to_col(ui: &egui::Ui, line: &str, x: f32, line_left: f32) -> usize {
    let target = (x - line_left).max(0.0);
    let cw = monospace_char_width(ui);
    let mut col = (target / cw).round() as isize;
    if col < 0 {
        col = 0;
    }
    let maxc = line.chars().count() as isize;
    (col.min(maxc)) as usize
}

// --- Key consumption helpers ---

fn mods_none() -> egui::Modifiers {
    egui::Modifiers::NONE
}
fn mods_shift() -> egui::Modifiers {
    let mut m = egui::Modifiers::NONE;
    m.shift = true;
    m
}
fn mods_ctrl() -> egui::Modifiers {
    let mut m = egui::Modifiers::NONE;
    m.ctrl = true;
    m
}
fn mods_ctrl_shift() -> egui::Modifiers {
    let mut m = egui::Modifiers::NONE;
    m.ctrl = true;
    m.shift = true;
    m
}
fn mods_cmd() -> egui::Modifiers {
    let mut m = egui::Modifiers::NONE;
    m.command = true;
    m
}
fn mods_cmd_shift() -> egui::Modifiers {
    let mut m = egui::Modifiers::NONE;
    m.command = true;
    m.shift = true;
    m
}

fn consume_key_variants(i: &mut egui::InputState, key: egui::Key) {
    let _ = i.consume_key(mods_none(), key);
    let _ = i.consume_key(mods_shift(), key);
    let _ = i.consume_key(mods_ctrl(), key);
    let _ = i.consume_key(mods_ctrl_shift(), key);
    let _ = i.consume_key(mods_cmd(), key);
    let _ = i.consume_key(mods_cmd_shift(), key);
}

/// Fast editor widget. Returns true if buffer changed this frame.
pub fn code_editor(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    theme: &egui_extras::syntax_highlighting::CodeTheme,
    id_source: &str,
    path_for_language: &str,
    text: &mut String,
    st: &mut CodeEditorState,
) -> bool {
    theme::seed_solarized_dark_once(ctx);
    st.begin_frame();

    let mut changed = false;

    let editor_id = egui::Id::new(("code_editor", id_source));

    let desired_size = ui.available_size();
    let (outer_rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    let outer_resp = ui.interact(outer_rect, editor_id, egui::Sense::click_and_drag());
    if outer_resp.clicked() || outer_resp.drag_started() {
        ui.memory_mut(|m| m.request_focus(editor_id));
        st.has_focus = true;
    } else {
        st.has_focus = ui.memory(|m| m.has_focus(editor_id));
    }

    // ---- Find/Replace bar ----
    let mut find_bar_height = 0.0;
    let mut pending_find_next = false;
    let mut pending_find_prev = false;
    let mut pending_replace = false;
    let mut pending_replace_all = false;

    ui.allocate_ui_at_rect(outer_rect, |ui| {
        ui.set_clip_rect(outer_rect);

        if st.find_open {
            let matches = find_all_matches(
                text,
                &st.find_query,
                st.find_match_case,
                st.find_whole_word,
            );
            let total = matches.len();
            let current = if total == 0 { 0 } else { st.find_match_index.min(total - 1) + 1 };

            egui::Frame::none()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Find:");

                        let find_resp = ui.add(
                            egui::TextEdit::singleline(&mut st.find_query).desired_width(220.0),
                        );

                        if find_resp.changed() {
                            st.find_match_index = 0;
                            st.active_find_match_cc = None;
                        }

                        ui.toggle_value(&mut st.find_match_case, "Aa")
                            .on_hover_text("Match case");
                        ui.toggle_value(&mut st.find_whole_word, "W")
                            .on_hover_text("Whole word");

                        if st.find_query.is_empty() || total == 0 {
                            ui.label("0/0");
                        } else {
                            ui.label(format!("{}/{}", current, total));
                        }

                        if find_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            pending_find_next = true;
                        }

                        ui.separator();

                        ui.label("Replace:");
                        ui.add(
                            egui::TextEdit::singleline(&mut st.replace_query).desired_width(220.0),
                        );

                        if ui.button("Prev").clicked() {
                            pending_find_prev = true;
                        }
                        if ui.button("Next").clicked() {
                            pending_find_next = true;
                        }

                        ui.separator();

                        if ui.button("Replace").clicked() {
                            pending_replace = true;
                        }
                        if ui.button("Replace all").clicked() {
                            pending_replace_all = true;
                        }

                        ui.separator();

                        if ui.button("✕").clicked() {
                            st.find_open = false;
                            st.active_find_match_cc = None;
                        }
                    });
                });

            find_bar_height = 36.0;
        }
    });

    let mut pending_copy = false;
    let mut pending_cut = false;
    let mut pending_scroll_pages: i32 = 0;

    // ---- Keyboard input (only when focused) ----
    if st.has_focus {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                editor_id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });

        let mut wants_repaint = false;

        ui.input_mut(|i| {
            let ctrl_down = i.modifiers.ctrl || i.modifiers.command;

            let (starts, lens) = compute_line_metrics(text);

            // PageUp / PageDown: SCROLL ONLY
            if i.key_pressed(egui::Key::PageUp) {
                pending_scroll_pages -= 1;
                consume_key_variants(i, egui::Key::PageUp);
                wants_repaint = true;
            }
            if i.key_pressed(egui::Key::PageDown) {
                pending_scroll_pages += 1;
                consume_key_variants(i, egui::Key::PageDown);
                wants_repaint = true;
            }

            // Arrow Left / Right
            if i.key_pressed(egui::Key::ArrowLeft) {
                st.active_find_match_cc = None;
                let shift = i.modifiers.shift;
                let ctrl = i.modifiers.ctrl || i.modifiers.command;
                if ctrl {
                    move_word_left(text, st, shift);
                } else {
                    move_cursor_left(text, st, shift);
                }
                consume_key_variants(i, egui::Key::ArrowLeft);
                wants_repaint = true;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                st.active_find_match_cc = None;
                let shift = i.modifiers.shift;
                let ctrl = i.modifiers.ctrl || i.modifiers.command;
                if ctrl {
                    move_word_right(text, st, shift);
                } else {
                    move_cursor_right(text, st, shift);
                }
                consume_key_variants(i, egui::Key::ArrowRight);
                wants_repaint = true;
            }

            // Arrow Up / Down
            if i.key_pressed(egui::Key::ArrowUp) {
                st.active_find_match_cc = None;
                let shift = i.modifiers.shift;
                let (line, col) = cc_to_lc_safe(text, st.cursor_cc);
                let target_line = line.saturating_sub(1);
                if !shift {
                    clear_selection(st);
                } else if st.selection_anchor_cc.is_none() {
                    st.selection_anchor_cc = Some(st.cursor_cc);
                }
                st.cursor_cc = lc_to_cc_metrics(&starts, &lens, target_line, col);
                consume_key_variants(i, egui::Key::ArrowUp);
                wants_repaint = true;
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                st.active_find_match_cc = None;
                let shift = i.modifiers.shift;
                let (line, col) = cc_to_lc_safe(text, st.cursor_cc);
                let max_line = lens.len().saturating_sub(1);
                let target_line = (line + 1).min(max_line);
                if !shift {
                    clear_selection(st);
                } else if st.selection_anchor_cc.is_none() {
                    st.selection_anchor_cc = Some(st.cursor_cc);
                }
                st.cursor_cc = lc_to_cc_metrics(&starts, &lens, target_line, col);
                consume_key_variants(i, egui::Key::ArrowDown);
                wants_repaint = true;
            }

            // Home / End
            if i.key_pressed(egui::Key::Home) {
                st.active_find_match_cc = None;
                let shift = i.modifiers.shift;
                let ctrl = i.modifiers.ctrl || i.modifiers.command;
                let (line, _col) = cc_to_lc_safe(text, st.cursor_cc);
                if !shift {
                    clear_selection(st);
                } else if st.selection_anchor_cc.is_none() {
                    st.selection_anchor_cc = Some(st.cursor_cc);
                }
                if ctrl {
                    st.cursor_cc = 0;
                } else {
                    st.cursor_cc = lc_to_cc_metrics(&starts, &lens, line, 0);
                }
                consume_key_variants(i, egui::Key::Home);
                wants_repaint = true;
            }
            if i.key_pressed(egui::Key::End) {
                st.active_find_match_cc = None;
                let shift = i.modifiers.shift;
                let ctrl = i.modifiers.ctrl || i.modifiers.command;
                let (line, _col) = cc_to_lc_safe(text, st.cursor_cc);
                if !shift {
                    clear_selection(st);
                } else if st.selection_anchor_cc.is_none() {
                    st.selection_anchor_cc = Some(st.cursor_cc);
                }
                if ctrl {
                    st.cursor_cc = cc_len(text);
                } else {
                    let line_len = lens.get(line).copied().unwrap_or(0);
                    st.cursor_cc = lc_to_cc_metrics(&starts, &lens, line, line_len);
                }
                consume_key_variants(i, egui::Key::End);
                wants_repaint = true;
            }

            // --- Event-driven editing / clipboard ---
            for ev in i.events.iter() {
                match ev {
                    egui::Event::Copy => pending_copy = true,
                    egui::Event::Cut => pending_cut = true,

                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        let shift = modifiers.shift;
                        let ctrl = modifiers.ctrl || modifiers.command;

                        match key {
                            egui::Key::Z if ctrl => {
                                st.active_find_match_cc = None;
                                if shift { st.redo(text); } else { st.undo(text); }
                                changed = true;
                                wants_repaint = true;
                            }
                            egui::Key::Y if ctrl => {
                                st.active_find_match_cc = None;
                                st.redo(text);
                                changed = true;
                                wants_repaint = true;
                            }

                            egui::Key::F if ctrl => {
                                st.find_open = true;
                                st.find_match_index = 0;
                                wants_repaint = true;
                            }
                            egui::Key::Escape => {
                                if st.find_open {
                                    st.find_open = false;
                                    st.active_find_match_cc = None;
                                    wants_repaint = true;
                                }
                            }

                            egui::Key::C if ctrl => pending_copy = true,
                            egui::Key::X if ctrl => pending_cut = true,

                            egui::Key::A if ctrl => {
                                st.active_find_match_cc = None;
                                set_selection(st, 0, cc_len(text));
                                wants_repaint = true;
                            }

                            egui::Key::Backspace => {
                                st.active_find_match_cc = None;
                                st.take_undo_snapshot_if_needed(text);
                                if ctrl { changed |= delete_prev_word(text, st); }
                                else { changed |= delete_prev_char(text, st); }
                                wants_repaint = true;
                            }
                            egui::Key::Delete => {
                                st.active_find_match_cc = None;
                                st.take_undo_snapshot_if_needed(text);
                                changed |= delete_next_char(text, st);
                                wants_repaint = true;
                            }
                            egui::Key::Enter => {
                                st.active_find_match_cc = None;
                                st.take_undo_snapshot_if_needed(text);
                                changed |= insert_text(text, st, "\n");
                                wants_repaint = true;
                            }
                            egui::Key::Tab => {
                                st.active_find_match_cc = None;
                                st.take_undo_snapshot_if_needed(text);
                                changed |= insert_text(text, st, "    ");
                                wants_repaint = true;
                            }

                            _ => {}
                        }
                    }

                    egui::Event::Text(t) => {
                        if ctrl_down {
                            continue;
                        }
                        if !t.is_empty() && !t.chars().any(|c| c == '\u{7f}') {
                            st.active_find_match_cc = None;
                            st.take_undo_snapshot_if_needed(text);
                            changed |= insert_text(text, st, t);
                            wants_repaint = true;
                        }
                    }

                    egui::Event::Paste(t) => {
                        st.active_find_match_cc = None;
                        st.take_undo_snapshot_if_needed(text);
                        changed |= insert_text(text, st, t);
                        wants_repaint = true;
                    }

                    _ => {}
                }
            }
        });

        if wants_repaint {
            ctx.request_repaint();
        }
    }

    // ---- Find/Replace actions ----
    if st.find_open && !st.find_query.is_empty() {
        let matches = find_all_matches(
            text,
            &st.find_query,
            st.find_match_case,
            st.find_whole_word,
        );

        if !matches.is_empty() {
            st.find_match_index = st.find_match_index.min(matches.len() - 1);
        } else {
            st.find_match_index = 0;
            st.active_find_match_cc = None;
        }

        if pending_find_next || pending_find_prev {
            if !matches.is_empty() {
                if pending_find_next {
                    st.find_match_index = (st.find_match_index + 1) % matches.len();
                } else {
                    st.find_match_index = if st.find_match_index == 0 {
                        matches.len() - 1
                    } else {
                        st.find_match_index - 1
                    };
                }
                let (a_bc, b_bc) = matches[st.find_match_index];
                select_match(text, st, a_bc, b_bc);

                request_scroll_to_cc(st, st.cursor_cc);
                ctx.request_repaint();
            }
        }

        if pending_replace {
            if !matches.is_empty() {
                let (a_bc, b_bc) = matches[st.find_match_index];
                st.take_undo_snapshot_if_needed(text);
                text.replace_range(a_bc..b_bc, &st.replace_query);
                changed = true;

                let new_b = a_bc + st.replace_query.len();
                select_match(text, st, a_bc, new_b);

                request_scroll_to_cc(st, st.cursor_cc);

                st.buffer_version = st.buffer_version.wrapping_add(1);
                st.line_cache.clear();
                ctx.request_repaint();
            }
        }

        if pending_replace_all {
            if !matches.is_empty() {
                st.take_undo_snapshot_if_needed(text);

                let mut out = String::with_capacity(text.len());
                let mut cursor = 0usize;
                for (a, b) in matches.iter() {
                    out.push_str(&text[cursor..*a]);
                    out.push_str(&st.replace_query);
                    cursor = *b;
                }
                out.push_str(&text[cursor..]);
                *text = out;

                clear_selection(st);
                st.cursor_cc = clamp_cc(text, st.cursor_cc);
                st.active_find_match_cc = None;

                changed = true;
                st.buffer_version = st.buffer_version.wrapping_add(1);
                st.line_cache.clear();
                ctx.request_repaint();
            }
        }
    }

    if changed {
        st.buffer_version = st.buffer_version.wrapping_add(1);
        st.line_cache.clear();
    }

    // ---- Render area below find bar ----
    let text_rect = if st.find_open {
        let top = outer_rect.top() + find_bar_height;
        egui::Rect::from_min_max(egui::pos2(outer_rect.left(), top), outer_rect.max)
    } else {
        outer_rect
    };

    let mut last_row_height: f32 = 16.0;
    let mut usable_scroll_height: f32 = (text_rect.height()).max(16.0);
    let mut did_scroll_to_cc = false;

    ui.allocate_ui_at_rect(text_rect, |ui| {
        ui.set_clip_rect(text_rect);

        let language = language_hint_for_path(path_for_language);
        let lines: Vec<&str> = text.split('\n').collect();

        let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(16.0);
        last_row_height = row_height;
        usable_scroll_height = (text_rect.height()).max(row_height);

        let cw = monospace_char_width(ui);

        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let pointer_pos = ui.input(|i| i.pointer.interact_pos());

        let out = egui::ScrollArea::both()
            .id_source(("code_editor_scroll", editor_id))
            .auto_shrink([false, false])
            .scroll_offset(st.scroll_offset)
            .show_rows(ui, row_height, lines.len().max(1), |ui, row_range| {
                let wrap_w = ui.available_width().max(1.0);
                let bucket: u32 = (wrap_w / 16.0).floor().max(1.0) as u32;

                let mut line_start_cc = 0usize;
                for i in 0..row_range.start.min(lines.len()) {
                    line_start_cc += lines[i].chars().count() + 1;
                }

                let (caret_line, _caret_col) = cc_to_lc_safe(text, st.cursor_cc);

                for row in row_range.clone() {
                    let line = lines.get(row).copied().unwrap_or("");
                    let line_len_cc = line.chars().count();
                    let line_end_cc = line_start_cc + line_len_cc;

                    let job = match st.line_cache.get(&row) {
                        Some((ver, b, job)) if *ver == st.buffer_version && *b == bucket => job.clone(),
                        _ => {
                            let mut job = highlight(ctx, theme, line, language);
                            job.wrap.max_width = wrap_w;
                            st.line_cache.insert(row, (st.buffer_version, bucket, job.clone()));
                            job
                        }
                    };

                    let w = ui.available_width().max(1.0);
                    let (row_rect, row_resp) = ui.allocate_exact_size(
                        egui::vec2(w, row_height),
                        egui::Sense::click_and_drag(),
                    );

                    if row_resp.clicked() || row_resp.drag_started() {
                        ui.memory_mut(|m| m.request_focus(editor_id));
                        st.has_focus = true;
                    }

                    // current line highlight
                    if st.has_focus && caret_line == row {
                        let mut line_fill = ui.visuals().selection.bg_fill;
                        line_fill = line_fill.linear_multiply(0.18);
                        ui.painter().rect_filled(row_rect, 0.0, line_fill);
                    }

                    // highlight all visible matches on this line (lighter)
                    if st.find_open && !st.find_query.is_empty() {
                        let mut match_fill = ui.visuals().selection.bg_fill;
                        match_fill = match_fill.linear_multiply(0.35);

                        for (a_col, b_col) in find_line_matches_cols(
                            line,
                            &st.find_query,
                            st.find_match_case,
                            st.find_whole_word,
                        ) {
                            let x1 = row_rect.left() + (a_col as f32) * cw;
                            let x2 = row_rect.left() + (b_col as f32) * cw;

                            let r = egui::Rect::from_min_max(
                                egui::pos2(x1, row_rect.top()),
                                egui::pos2(x2, row_rect.bottom()),
                            );

                            ui.painter().rect_filled(r, 0.0, match_fill);
                        }
                    }

                    //  selection background: if it's the "active find match", use yellow
                    if let Some((a, b)) = selection_range(st) {
                        let sa = a.max(line_start_cc).min(line_end_cc);
                        let sb = b.max(line_start_cc).min(line_end_cc);
                        if sa < sb {
                            let a_col = sa - line_start_cc;
                            let b_col = sb - line_start_cc;

                            let x1 = row_rect.left() + (a_col as f32) * cw;
                            let x2 = row_rect.left() + (b_col as f32) * cw;

                            let sel_rect = egui::Rect::from_min_max(
                                egui::pos2(x1, row_rect.top()),
                                egui::pos2(x2, row_rect.bottom()),
                            );

                            let is_active_find = st
                                .active_find_match_cc
                                .map(|(fa, fb)| fa == a && fb == b)
                                .unwrap_or(false);

                            let fill = if st.find_open && is_active_find {
                                egui::Color32::from_rgba_unmultiplied(255, 215, 0, 140) // gold-ish
                            } else {
                                ui.visuals().selection.bg_fill
                            };

                            ui.painter().rect_filled(sel_rect, 0.0, fill);
                        }
                    }

                    // draw highlighted line
                    let galley = ui.fonts(|f| f.layout_job(job));
                    ui.painter().galley(row_rect.left_top(), galley, ui.visuals().text_color());

                    // click / drag start
                    if (row_resp.clicked() || row_resp.drag_started()) && st.has_focus {
                        if let Some(pos) = pointer_pos {
                            if row_rect.contains(pos) {
                                // any manual selection clears "active find match"
                                st.active_find_match_cc = None;

                                let col = click_to_col(ui, line, pos.x, row_rect.left());
                                let new_cc = (line_start_cc + col).min(line_end_cc);

                                let shift = ui.input(|i| i.modifiers.shift);

                                if shift {
                                    if st.selection_anchor_cc.is_none() {
                                        st.selection_anchor_cc = Some(st.cursor_cc);
                                    }
                                    st.cursor_cc = new_cc;
                                } else {
                                    st.cursor_cc = new_cc;
                                    clear_selection(st);

                                    if row_resp.drag_started() {
                                        st.mouse_selecting = true;
                                        st.mouse_anchor_cc = Some(new_cc);
                                        st.selection_anchor_cc = Some(new_cc);
                                    } else {
                                        st.mouse_selecting = false;
                                        st.mouse_anchor_cc = None;
                                    }
                                }

                                ctx.request_repaint();
                            }
                        }
                    }

                    // drag selection
                    if st.mouse_selecting && pointer_down {
                        if let Some(pos) = pointer_pos {
                            if row_rect.contains(pos) {
                                st.active_find_match_cc = None;

                                let col = click_to_col(ui, line, pos.x, row_rect.left());
                                let new_cc = (line_start_cc + col).min(line_end_cc);

                                if let Some(anchor) = st.mouse_anchor_cc {
                                    st.selection_anchor_cc = Some(anchor);
                                } else if st.selection_anchor_cc.is_none() {
                                    st.selection_anchor_cc = Some(st.cursor_cc);
                                }

                                st.cursor_cc = new_cc;
                                ctx.request_repaint();
                            }
                        }
                    }

                    // end selection
                    if st.mouse_selecting && !pointer_down {
                        st.mouse_selecting = false;
                        st.mouse_anchor_cc = None;

                        if let Some((a, b)) = selection_range(st) {
                            if a == b {
                                clear_selection(st);
                            }
                        }
                    }

                    // caret
                    if st.has_focus {
                        let cur_cc = clamp_cc(text, st.cursor_cc);
                        if cur_cc >= line_start_cc && cur_cc <= line_end_cc {
                            let col = cur_cc - line_start_cc;
                            let caret_x = row_rect.left() + (col as f32) * cw;
                            let caret_rect = egui::Rect::from_min_max(
                                egui::pos2(caret_x, row_rect.top()),
                                egui::pos2(caret_x + 1.0, row_rect.bottom()),
                            );
                            ui.painter().rect_filled(caret_rect, 0.0, ui.visuals().text_color());
                        }
                    }

                    line_start_cc = line_end_cc + 1;
                }
            });

        st.scroll_offset = out.state.offset;

        if apply_scroll_to_cc_if_needed(text, st, row_height, usable_scroll_height) {
            did_scroll_to_cc = true;
        }
    });

    if pending_scroll_pages != 0 {
        let overlap = (last_row_height * 2.0).min(usable_scroll_height * 0.25);
        let page = (usable_scroll_height - overlap).max(last_row_height);

        st.scroll_offset.y = (st.scroll_offset.y + (pending_scroll_pages as f32) * page).max(0.0);
        ctx.request_repaint();
    }

    if did_scroll_to_cc {
        ctx.request_repaint();
    }

    // Clipboard ops AFTER selection updated
    if pending_copy {
        if let Some((a, b)) = selection_range(st) {
            if a != b {
                let a_bc = cc_to_bc(text, a);
                let b_bc = cc_to_bc(text, b);
                ctx.copy_text(text[a_bc..b_bc].to_string());
            }
        }
    }

    if pending_cut {
        if let Some((a, b)) = selection_range(st) {
            if a != b {
                let a_bc = cc_to_bc(text, a);
                let b_bc = cc_to_bc(text, b);
                ctx.copy_text(text[a_bc..b_bc].to_string());

                st.take_undo_snapshot_if_needed(text);
                if delete_selection(text, st) {
                    changed = true;
                    st.buffer_version = st.buffer_version.wrapping_add(1);
                    st.line_cache.clear();
                    ctx.request_repaint();
                }
            }
        }
    }

    st.cursor_cc = clamp_cc(text, st.cursor_cc);
    changed
}
