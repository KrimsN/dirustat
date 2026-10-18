//! The left directory tree: columns (name, size, % of parent, files, modified), expand/collapse,
//! sorting, virtualized rows and selection sync with `Session`.
//!
//! The visible rows are a flattened list rebuilt only when the tree revision, the size mode, the
//! sort or the expansion set change (`flatten` is pure and iterative). Only the rows in view are
//! drawn (`ScrollArea::show_rows`).

use crate::app::human_size;
use crate::state::UiState;
use crate::theme::Tokens;
use dirustat_engine::Session;
use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Hard cap on flattened rows: a hostile or huge expansion cannot blow up memory or frame time.
pub const MAX_ROWS: usize = 500_000;

/// Cap on the expansion set kept in the state (entries beyond it are ignored when inserting).
pub const MAX_EXPANDED: usize = 1_000_000;

const INDENT: f32 = 14.0;
const SIZE_W: f32 = 76.0;
const BAR_W: f32 = 80.0;
const FILES_W: f32 = 56.0;
const DATE_W: f32 = 84.0;
const NAME_MIN_W: f32 = 110.0;
const ROW_PAD: f32 = 4.0;

/// A sortable column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeColumn {
    Name,
    Size,
    Percent,
    Files,
    Modified,
}

/// Sort order of the tree; applied to the children of every expanded folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeSort {
    pub column: TreeColumn,
    pub descending: bool,
}

impl Default for TreeSort {
    /// Largest first.
    fn default() -> Self {
        Self { column: TreeColumn::Size, descending: true }
    }
}

impl TreeSort {
    /// Clicking a column: the same column flips the direction, a new column starts with its natural
    /// direction (names ascending, everything else descending).
    #[must_use]
    pub fn toggled(self, column: TreeColumn) -> Self {
        if self.column == column {
            Self { column, descending: !self.descending }
        } else {
            Self { column, descending: column != TreeColumn::Name }
        }
    }
}

/// One visible row of the flattened tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub node: NodeId,
    pub depth: u32,
    /// The node is a folder with at least one child.
    pub expandable: bool,
    pub expanded: bool,
}

/// Result of flattening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flat {
    pub rows: Vec<Row>,
    /// The row cap was hit; the rest is not shown.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Num(i128),
    Text(String),
}

fn sort_key(tree: &Tree, id: NodeId, sort: TreeSort, mode: SizeMode) -> SortKey {
    let Some(node) = tree.get(id) else { return SortKey::Num(i128::MIN) };
    match sort.column {
        TreeColumn::Name => SortKey::Text(node.name().to_lowercase()),
        TreeColumn::Size | TreeColumn::Percent => SortKey::Num(i128::from(node.total_size(mode))),
        TreeColumn::Files => SortKey::Num(i128::from(node.file_count())),
        TreeColumn::Modified => SortKey::Num(node.data().mtime.map_or(i128::MIN, i128::from)),
    }
}

/// Children of `parent` in display order. The sort is stable, so equal keys keep scan order in both
/// directions.
pub fn sorted_children(tree: &Tree, parent: NodeId, sort: TreeSort, mode: SizeMode) -> Vec<NodeId> {
    let mut keyed: Vec<(SortKey, NodeId)> = tree.children(parent).map(|c| (sort_key(tree, c, sort, mode), c)).collect();
    if sort.descending {
        keyed.sort_by(|a, b| b.0.cmp(&a.0));
    } else {
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
    }
    keyed.into_iter().map(|(_, id)| id).collect()
}

/// Flattens the visible part of the tree into rows (pre-order). Iterative; unknown ids in `expanded`
/// are ignored; at most `max_rows` rows are produced.
pub fn flatten(tree: &Tree, expanded: &BTreeSet<usize>, sort: TreeSort, mode: SizeMode, max_rows: usize) -> Flat {
    let mut flat = Flat::default();
    if tree.is_empty() {
        return flat;
    }
    let mut stack: Vec<(NodeId, u32)> = vec![(tree.root(), 0)];
    while let Some((id, depth)) = stack.pop() {
        if flat.rows.len() >= max_rows {
            flat.truncated = true;
            break;
        }
        let Some(node) = tree.get(id) else { continue };
        let expandable = node.kind() == NodeKind::Dir && tree.children(id).next().is_some();
        let is_expanded = expandable && expanded.contains(&id.index());
        flat.rows.push(Row { node: id, depth, expandable, expanded: is_expanded });
        if is_expanded {
            let child_depth = depth.saturating_add(1);
            for child in sorted_children(tree, id, sort, mode).into_iter().rev() {
                stack.push((child, child_depth));
            }
        }
    }
    flat
}

/// Ancestors of `id` (nearest first, `id` itself excluded), bounded by `cap` steps.
pub fn ancestors(tree: &Tree, id: NodeId, cap: usize) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = tree.get(id).and_then(|n| n.parent());
    while let Some(p) = cur {
        if out.len() >= cap {
            break;
        }
        out.push(p);
        cur = tree.get(p).and_then(|n| n.parent());
    }
    out
}

/// `part / whole` in tenths of a percent, clamped to 0..=1000; 0 when `whole` is 0.
pub fn percent_tenths(part: u64, whole: u64) -> u32 {
    if whole == 0 {
        return 0;
    }
    let t = u128::from(part).saturating_mul(1000) / u128::from(whole);
    u32::try_from(t.min(1000)).unwrap_or(1000)
}

fn percent_text(tenths: u32) -> String {
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// `YYYY-MM-DD` (UTC) for seconds since the Unix epoch; `-` when unknown or out of range.
pub fn format_date(secs: Option<i64>) -> String {
    let Some(secs) = secs else { return "-".to_owned() };
    let days = secs.div_euclid(86_400);
    // Years 0000..=9999 only; this also keeps the arithmetic below far from overflow.
    if !(-719_468..=2_932_896).contains(&days) {
        return "-".to_owned();
    }
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Cached flattened rows. Not serialized and ignored by `UiState` equality.
#[derive(Debug, Default)]
pub struct TreeCache {
    key: Option<CacheKey>,
    flat: Flat,
    /// Selection seen on the previous frame, to detect changes made elsewhere (treemap, control).
    last_selection: Option<NodeId>,
}

impl Clone for TreeCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for TreeCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for TreeCache {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    mode: SizeMode,
    sort: TreeSort,
    expanded: BTreeSet<usize>,
}

impl TreeCache {
    /// Number of times the rows were rebuilt is observable through `rows()` identity in tests.
    pub fn rows(&self) -> &[Row] {
        &self.flat.rows
    }
}

/// Brings the cache up to date; returns true when rows were rebuilt.
fn refresh(cache: &mut TreeCache, tree: &Tree, state: &UiState, revision: u64, mode: SizeMode) -> bool {
    let fresh = matches!(&cache.key, Some(k)
        if k.revision == revision && k.mode == mode && k.sort == state.tree_sort && k.expanded == state.tree_expanded);
    if fresh {
        return false;
    }
    cache.flat = flatten(tree, &state.tree_expanded, state.tree_sort, mode, MAX_ROWS);
    cache.key = Some(CacheKey { revision, mode, sort: state.tree_sort, expanded: state.tree_expanded.clone() });
    true
}

/// Resets the expansion when the tree was replaced (node indices of the old tree are meaningless);
/// the root starts expanded.
fn sync_revision(state: &mut UiState, tree: &Tree, revision: u64) {
    if state.tree_revision != Some(revision) {
        state.tree_revision = Some(revision);
        state.tree_expanded.clear();
        if !tree.is_empty() {
            state.tree_expanded.insert(tree.root().index());
        }
    }
}

/// Expands the ancestors of `id`; returns true when the set changed.
fn reveal(state: &mut UiState, tree: &Tree, id: NodeId) -> bool {
    let mut changed = false;
    for a in ancestors(tree, id, 100_000) {
        if state.tree_expanded.len() >= MAX_EXPANDED {
            break;
        }
        changed |= state.tree_expanded.insert(a.index());
    }
    changed
}

fn toggle(state: &mut UiState, id: NodeId) {
    let idx = id.index();
    if !state.tree_expanded.remove(&idx) && state.tree_expanded.len() < MAX_EXPANDED {
        state.tree_expanded.insert(idx);
    }
}

/// Column widths of the tree table. Name takes the flexible rest; when the panel is too narrow
/// for a readable Name, the optional columns are dropped (width 0) in the order Modified, Files,
/// % of parent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Widths {
    name: f32,
    bar: f32,
    files: f32,
    date: f32,
}

impl Widths {
    fn new(available: f32) -> Self {
        let avail = if available.is_finite() { available } else { 0.0 };
        let mut optional = [DATE_W, FILES_W, BAR_W];
        let mut fixed = SIZE_W + optional.iter().sum::<f32>();
        for slot in &mut optional {
            if avail - fixed >= NAME_MIN_W {
                break;
            }
            fixed -= *slot;
            *slot = 0.0;
        }
        let [date, files, bar] = optional;
        Self { name: (avail - fixed).max(NAME_MIN_W), bar, files, date }
    }

    fn total(self) -> f32 {
        self.name + SIZE_W + self.bar + self.files + self.date
    }

    /// Cell rectangles for a row rectangle: name, size, bar, files, modified.
    fn cells(self, row: egui::Rect) -> [egui::Rect; 5] {
        let mut x = row.left();
        let mut next = |w: f32| {
            let r = egui::Rect::from_min_max(egui::pos2(x, row.top()), egui::pos2(x + w, row.bottom()));
            x += w;
            r
        };
        [next(self.name), next(SIZE_W), next(self.bar), next(self.files), next(self.date)]
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Arrow {
    Right,
    Down,
    Up,
}

pub(crate) fn arrow(painter: &egui::Painter, center: egui::Pos2, half: f32, dir: Arrow, color: egui::Color32) {
    let h = half;
    let pts = match dir {
        Arrow::Right => {
            [center + egui::vec2(-h * 0.6, -h), center + egui::vec2(-h * 0.6, h), center + egui::vec2(h, 0.0)]
        }
        Arrow::Down => {
            [center + egui::vec2(-h, -h * 0.6), center + egui::vec2(h, -h * 0.6), center + egui::vec2(0.0, h)]
        }
        Arrow::Up => [center + egui::vec2(-h, h * 0.6), center + egui::vec2(h, h * 0.6), center + egui::vec2(0.0, -h)],
    };
    painter.add(egui::Shape::convex_polygon(pts.to_vec(), color, egui::Stroke::NONE));
}

/// What the user did during a frame.
#[derive(Default)]
struct Actions {
    select: Option<NodeId>,
    toggle: Option<NodeId>,
    sort: Option<TreeColumn>,
}

fn header(ui: &mut egui::Ui, widths: Widths, state: &UiState, tokens: &Tokens, actions: &mut Actions) {
    let row_h = tokens.font_body + ROW_PAD * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(widths.total(), row_h), egui::Sense::hover());
    let cells = widths.cells(rect);
    let columns = [
        (TreeColumn::Name, "Name"),
        (TreeColumn::Size, "Size"),
        (TreeColumn::Percent, "% of parent"),
        (TreeColumn::Files, "Files"),
        (TreeColumn::Modified, "Modified"),
    ];
    let font = egui::FontId::proportional(tokens.font_small);
    ui.painter().rect_filled(rect, 0.0, Tokens::color(tokens.surface));
    for ((column, label), cell) in columns.into_iter().zip(cells) {
        let resp = ui.interact(cell, ui.id().with(("tree_header", label)), egui::Sense::click());
        if resp.clicked() {
            actions.sort = Some(column);
        }
        let painter = ui.painter_at(cell);
        let color = if state.tree_sort.column == column { tokens.accent } else { tokens.text };
        let color = Tokens::color(color);
        painter.text(
            cell.left_center() + egui::vec2(ROW_PAD, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
        if state.tree_sort.column == column {
            let dir = if state.tree_sort.descending { Arrow::Down } else { Arrow::Up };
            arrow(&painter, cell.right_center() - egui::vec2(ROW_PAD + 4.0, 0.0), 3.5, dir, color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    ui: &mut egui::Ui,
    tree: &Tree,
    row: Row,
    widths: Widths,
    row_h: f32,
    mode: SizeMode,
    selected: Option<NodeId>,
    tokens: &Tokens,
    actions: &mut Actions,
) {
    let Some(node) = tree.get(row.node) else { return };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(widths.total(), row_h), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    if selected == Some(row.node) {
        painter.rect_filled(rect, 0.0, Tokens::color(tokens.accent).gamma_multiply(0.35));
    } else if resp.hovered() {
        painter.rect_filled(rect, 0.0, Tokens::color(tokens.surface));
    }
    if resp.clicked() {
        actions.select = Some(row.node);
    }
    if resp.double_clicked() && row.expandable {
        actions.toggle = Some(row.node);
    }
    let [name_cell, size_cell, bar_cell, files_cell, date_cell] = widths.cells(rect);
    let font = egui::FontId::proportional(tokens.font_body);
    let text = Tokens::color(tokens.text);
    let muted = Tokens::color(tokens.text_muted);

    // Name with indent and expander arrow.
    let indent = (f32::from(u16::try_from(row.depth).unwrap_or(u16::MAX)) * INDENT).min(widths.name * 0.6);
    let arrow_c = egui::pos2(name_cell.left() + ROW_PAD + indent + INDENT * 0.5, rect.center().y);
    if row.expandable {
        let hit = egui::Rect::from_center_size(arrow_c, egui::vec2(INDENT, row_h));
        let arrow_resp = ui.interact(hit, resp.id.with("expander"), egui::Sense::click());
        if arrow_resp.clicked() {
            actions.toggle = Some(row.node);
            actions.select = None;
        }
        arrow(&painter, arrow_c, 3.5, if row.expanded { Arrow::Down } else { Arrow::Right }, muted);
    }
    let name_color = if node.data().error { Tokens::color(tokens.error) } else { text };
    let name_pos = egui::pos2(arrow_c.x + INDENT * 0.5 + 2.0, rect.center().y);
    let name_rect = painter.with_clip_rect(name_cell).text(
        name_pos,
        egui::Align2::LEFT_CENTER,
        node.name(),
        font.clone(),
        name_color,
    );
    if name_rect.right() > name_cell.right() {
        // The name does not fit the column: the tooltip shows it in full.
        resp.clone().on_hover_text(node.name());
    }

    // Size.
    let total = node.total_size(mode);
    painter.with_clip_rect(size_cell).text(
        egui::pos2(size_cell.right() - ROW_PAD, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        human_size(total),
        font.clone(),
        text,
    );

    // Share of the parent (the column is dropped in narrow panels).
    let parent_total = node.parent().and_then(|p| tree.get(p)).map_or(total, |p| p.total_size(mode));
    if widths.bar > 0.0 {
        let tenths = percent_tenths(total, parent_total);
        let track = bar_cell.shrink2(egui::vec2(ROW_PAD, 3.0));
        painter.rect_filled(track, f32::from(tokens.radius_small), Tokens::color(tokens.background));
        let fill_w = track.width() * f32::from(u16::try_from(tenths).unwrap_or(1000)) / 1000.0;
        if fill_w > 0.5 {
            let fill = egui::Rect::from_min_size(track.min, egui::vec2(fill_w, track.height()));
            painter.rect_filled(fill, f32::from(tokens.radius_small), Tokens::color(tokens.accent));
        }
        painter.with_clip_rect(bar_cell).text(
            track.center(),
            egui::Align2::CENTER_CENTER,
            percent_text(tenths),
            egui::FontId::proportional(tokens.font_small),
            text,
        );
    }

    // Files and modification time.
    let files = if node.kind() == NodeKind::Dir { node.file_count().to_string() } else { String::new() };
    painter.with_clip_rect(files_cell).text(
        egui::pos2(files_cell.right() - ROW_PAD, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        files,
        font.clone(),
        muted,
    );
    painter.with_clip_rect(date_cell).text(
        egui::pos2(date_cell.left() + ROW_PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format_date(node.data().mtime),
        font,
        muted,
    );
}

/// Draws the panel.
pub fn show(ui: &mut egui::Ui, session: &mut Session, state: &mut UiState, tokens: &Tokens) {
    let revision = session.revision();
    let mode = session.map_size_mode();
    sync_revision(state, session.tree(), revision);

    // Selection changed elsewhere (treemap, control): expand its ancestors and scroll to it.
    let selection = session.selection();
    let mut scroll_to = None;
    if selection != state.tree_cache.last_selection {
        state.tree_cache.last_selection = selection;
        if let Some(sel) = selection {
            reveal(state, session.tree(), sel);
            scroll_to = Some(sel);
        }
    }

    let mut cache = std::mem::take(&mut state.tree_cache);
    refresh(&mut cache, session.tree(), state, revision, mode);

    let row_h = tokens.font_body + ROW_PAD * 2.0;
    let widths = Widths::new(ui.available_width());
    let mut actions = Actions::default();

    if session.tree().len() <= 1 {
        ui.weak("No data. Enter a folder path and press Scan.");
    }
    header(ui, widths, state, tokens, &mut actions);

    let total_rows = cache.flat.rows.len();
    let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
    if let Some(sel) = scroll_to
        && let Some(i) = cache.flat.rows.iter().position(|r| r.node == sel)
    {
        area = area.vertical_scroll_offset((i as f32 * row_h - row_h * 2.0).max(0.0));
    }
    let tree = session.tree();
    area.show_rows(ui, row_h, total_rows, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in cache.flat.rows.get(range).unwrap_or_default() {
            draw_row(ui, tree, *row, widths, row_h, mode, selection, tokens, &mut actions);
        }
    });
    if cache.flat.truncated {
        ui.colored_label(Tokens::color(tokens.warning), format!("Showing the first {MAX_ROWS} rows"));
    }

    if let Some(column) = actions.sort {
        state.tree_sort = state.tree_sort.toggled(column);
    }
    if let Some(id) = actions.toggle {
        toggle(state, id);
    }
    if let Some(id) = actions.select
        && session.execute("select.node", &serde_json::json!({ "id": id.index() })).is_ok()
    {
        cache.last_selection = Some(id);
    }
    state.tree_cache = cache;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn build_tree() -> Tree {
        let mut tree = Tree::new("root");
        let root = tree.root();
        let d = tree.add_child(root, "Dir", NodeKind::Dir, 0).unwrap();
        tree.add_child(d, "big", NodeKind::File, 600).unwrap();
        tree.add_child(d, "small", NodeKind::File, 100).unwrap();
        tree.add_child(root, "other", NodeKind::File, 300).unwrap();
        tree.add_child(root, "alpha", NodeKind::File, 300).unwrap();
        tree.aggregate();
        tree
    }

    #[test]
    fn widths_drop_optional_columns_before_squeezing_the_name() {
        let wide = Widths::new(1000.0);
        assert!(wide.bar > 0.0 && wide.files > 0.0 && wide.date > 0.0);
        assert!((wide.total() - 1000.0).abs() < 1e-3, "name takes the flexible rest");
        let mid = Widths::new(330.0);
        assert!(mid.date == 0.0 && mid.name >= NAME_MIN_W);
        let narrow = Widths::new(190.0);
        assert!(narrow.bar == 0.0 && narrow.files == 0.0 && narrow.date == 0.0 && narrow.name >= NAME_MIN_W);
        assert!(Widths::new(f32::NAN).name >= NAME_MIN_W);
    }

    fn names(tree: &Tree, flat: &Flat) -> Vec<String> {
        flat.rows.iter().filter_map(|r| tree.get(r.node)).map(|n| n.name().to_owned()).collect()
    }

    fn all_expanded(tree: &Tree) -> BTreeSet<usize> {
        (0..tree.len()).collect()
    }

    #[test]
    fn size_desc_is_default_and_stable_on_ties() {
        let tree = build_tree();
        let flat = flatten(&tree, &all_expanded(&tree), TreeSort::default(), SizeMode::Logical, MAX_ROWS);
        // Dir (700), then other/alpha (300, scan order kept), children of Dir sorted too.
        assert_eq!(names(&tree, &flat), ["root", "Dir", "big", "small", "other", "alpha"]);
        let asc = TreeSort { column: TreeColumn::Size, descending: false };
        let flat = flatten(&tree, &all_expanded(&tree), asc, SizeMode::Logical, MAX_ROWS);
        assert_eq!(names(&tree, &flat), ["root", "other", "alpha", "Dir", "small", "big"]);
    }

    #[test]
    fn name_sort_is_case_insensitive_and_depth_is_tracked() {
        let tree = build_tree();
        let sort = TreeSort { column: TreeColumn::Name, descending: false };
        let flat = flatten(&tree, &all_expanded(&tree), sort, SizeMode::Logical, MAX_ROWS);
        assert_eq!(names(&tree, &flat), ["root", "alpha", "Dir", "big", "small", "other"]);
        let depths: Vec<u32> = flat.rows.iter().map(|r| r.depth).collect();
        assert_eq!(depths, [0, 1, 1, 2, 2, 1]);
    }

    #[test]
    fn collapsed_children_are_hidden_and_flags_are_set() {
        let tree = build_tree();
        let only_root: BTreeSet<usize> = [tree.root().index()].into();
        let flat = flatten(&tree, &only_root, TreeSort::default(), SizeMode::Logical, MAX_ROWS);
        assert_eq!(flat.rows.len(), 4);
        let dir_row = flat.rows.iter().find(|r| tree.get(r.node).is_some_and(|n| n.name() == "Dir")).unwrap();
        assert!(dir_row.expandable && !dir_row.expanded);
        let file_row = flat.rows.iter().find(|r| tree.get(r.node).is_some_and(|n| n.name() == "other")).unwrap();
        assert!(!file_row.expandable);
    }

    #[test]
    fn empty_tree_and_hostile_expansion_sets() {
        let empty = Tree::new("r");
        let flat = flatten(&empty, &BTreeSet::new(), TreeSort::default(), SizeMode::Logical, MAX_ROWS);
        assert_eq!(flat.rows.len(), 1);
        let tree = build_tree();
        let hostile: BTreeSet<usize> = [usize::MAX, 9_999_999, 0].into();
        let flat = flatten(&tree, &hostile, TreeSort::default(), SizeMode::Logical, MAX_ROWS);
        assert_eq!(flat.rows.len(), 4);
        // Expanding a file is ignored.
        let file = tree.children(tree.root()).find(|c| tree.get(*c).is_some_and(|n| n.name() == "other")).unwrap();
        let on_file: BTreeSet<usize> = [0, file.index()].into();
        assert_eq!(flatten(&tree, &on_file, TreeSort::default(), SizeMode::Logical, MAX_ROWS).rows.len(), 4);
        // Row cap.
        let capped = flatten(&tree, &all_expanded(&tree), TreeSort::default(), SizeMode::Logical, 3);
        assert_eq!(capped.rows.len(), 3);
        assert!(capped.truncated);
        assert!(flatten(&tree, &all_expanded(&tree), TreeSort::default(), SizeMode::Logical, 0).rows.is_empty());
    }

    #[test]
    fn million_nodes_flatten_is_bounded_by_expanded_rows() {
        let mut tree = Tree::new("root");
        let root = tree.root();
        for i in 0..1000 {
            let d = tree.add_child(root, &format!("d{i}"), NodeKind::Dir, 0).unwrap();
            for j in 0..1000_u64 {
                tree.add_child(d, &format!("f{j}"), NodeKind::File, j % 97 + 1).unwrap();
            }
        }
        tree.aggregate();
        assert!(tree.len() > 1_000_000);
        let start = Instant::now();
        let only_root: BTreeSet<usize> = [root.index()].into();
        let flat = flatten(&tree, &only_root, TreeSort::default(), SizeMode::Allocated, MAX_ROWS);
        assert_eq!(flat.rows.len(), 1001);
        let first = tree.children(root).next().unwrap();
        let two: BTreeSet<usize> = [root.index(), first.index()].into();
        let flat = flatten(&tree, &two, TreeSort::default(), SizeMode::Allocated, MAX_ROWS);
        assert_eq!(flat.rows.len(), 2001);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn toggled_sort_and_helpers() {
        let s = TreeSort::default();
        assert!(!s.toggled(TreeColumn::Size).descending);
        assert_eq!(s.toggled(TreeColumn::Name), TreeSort { column: TreeColumn::Name, descending: false });
        assert_eq!(percent_tenths(1, 0), 0);
        assert_eq!(percent_tenths(5, 4), 1000);
        assert_eq!(percent_tenths(1, 3), 333);
        assert_eq!(percent_tenths(u64::MAX, u64::MAX), 1000);
        assert_eq!(percent_text(333), "33.3%");
    }

    #[test]
    fn dates_format_and_hostile_values_are_dashes() {
        assert_eq!(format_date(Some(0)), "1970-01-01");
        assert_eq!(format_date(Some(951_782_400)), "2000-02-29");
        assert_eq!(format_date(Some(-1)), "1969-12-31");
        assert_eq!(format_date(None), "-");
        assert_eq!(format_date(Some(i64::MIN)), "-");
        assert_eq!(format_date(Some(i64::MAX)), "-");
    }

    #[test]
    fn ancestors_are_bounded_and_ordered() {
        let tree = build_tree();
        let big = tree.children(tree.children(tree.root()).next().unwrap()).next().unwrap();
        let a = ancestors(&tree, big, 100);
        assert_eq!(a.len(), 2);
        assert_eq!(a.last().copied(), Some(tree.root()));
        assert_eq!(ancestors(&tree, big, 1).len(), 1);
        assert!(ancestors(&tree, tree.root(), 10).is_empty());
    }

    fn scanned_session() -> Option<(tempfile::TempDir, Session)> {
        let dir = tempfile::tempdir().ok()?;
        std::fs::create_dir(dir.path().join("sub")).ok()?;
        std::fs::write(dir.path().join("sub").join("a.bin"), vec![1u8; 5000]).ok()?;
        std::fs::write(dir.path().join("b.bin"), vec![1u8; 100]).ok()?;
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).ok()?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut session = Session::new();
        session.finish_scan(handle).ok()?;
        Some((dir, session))
    }

    fn frame(ctx: &egui::Context, session: &mut Session, state: &mut UiState, tokens: &Tokens) -> usize {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui, session, state, tokens));
        let n = out.shapes.len();
        out.drop_without_applying_deltas();
        n
    }

    #[test]
    fn headless_frame_with_data_caches_rows_and_syncs_selection() {
        let Some((_dir, mut session)) = scanned_session() else { return };
        let ctx = egui::Context::default();
        let mut state = UiState::default();
        let tokens = Tokens::default();
        assert!(frame(&ctx, &mut session, &mut state, &tokens) > 0);
        assert!(state.tree_expanded.contains(&session.tree().root().index()), "root starts expanded");
        let rows = state.tree_cache.rows().len();
        assert!(rows >= 3, "{rows}");
        let ptr = state.tree_cache.rows().as_ptr();
        frame(&ctx, &mut session, &mut state, &tokens);
        assert_eq!(state.tree_cache.rows().as_ptr(), ptr, "rows are reused while nothing changed");

        // Selecting a deep node elsewhere expands its ancestors.
        let sub = session
            .tree()
            .children(session.tree().root())
            .find(|c| session.tree().get(*c).is_some_and(|n| n.kind() == NodeKind::Dir));
        let Some(sub) = sub else { return };
        let Some(leaf) = session.tree().children(sub).next() else { return };
        assert!(session.select_node(leaf));
        frame(&ctx, &mut session, &mut state, &tokens);
        assert!(state.tree_expanded.contains(&sub.index()));
        assert!(state.tree_cache.rows().iter().any(|r| r.node == leaf));

        // Sorting change rebuilds; both themes draw.
        state.tree_sort = TreeSort { column: TreeColumn::Name, descending: false };
        state.tree_expanded.insert(usize::MAX);
        for kind in [crate::ThemeKind::Light, crate::ThemeKind::Dark] {
            assert!(frame(&ctx, &mut session, &mut state, &Tokens::for_kind(kind)) > 0);
        }
        let json = serde_json::to_string(&state).unwrap_or_default();
        assert!(json.contains("tree_expanded"));
    }

    #[test]
    fn empty_session_frame_is_safe() {
        let ctx = egui::Context::default();
        let mut session = Session::new();
        let mut state = UiState::default();
        assert!(frame(&ctx, &mut session, &mut state, &Tokens::default()) > 0);
    }
}
