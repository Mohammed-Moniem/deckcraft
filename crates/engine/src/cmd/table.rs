//! Table Design and Table Layout tabs.

use deckcraft_geom::{Affine, Point, Rect};
use deckcraft_model::{Shape, ShapeKind, Table};
use serde_json::{Value, json};

use super::{
    CommandSpec, bad, bool_or, bool_param, cmd, color_param, edit_shapes, f64_param, has_selection, has_slide, id_param, ok, opt_usize_param,
    str_param, table_drawn_heights, xfrm_of,
};
use crate::{DocState, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("table.insertRowAbove", "Insert Above", ["Table Layout", "Rows & Columns"], None, "{row?, id?}", has_selection, |s, p| rows_cols(
            s,
            p,
            "rowAbove",
            "table.insertRowAbove"
        )),
        cmd!("table.insertRowBelow", "Insert Below", ["Table Layout", "Rows & Columns"], None, "{row?, id?}", has_selection, |s, p| rows_cols(
            s,
            p,
            "rowBelow",
            "table.insertRowBelow"
        )),
        cmd!("table.insertColumnLeft", "Insert Left", ["Table Layout", "Rows & Columns"], None, "{col?, id?}", has_selection, |s, p| rows_cols(
            s,
            p,
            "colLeft",
            "table.insertColumnLeft"
        )),
        cmd!("table.insertColumnRight", "Insert Right", ["Table Layout", "Rows & Columns"], None, "{col?, id?}", has_selection, |s, p| rows_cols(
            s,
            p,
            "colRight",
            "table.insertColumnRight"
        )),
        cmd!("table.deleteRow", "Delete Rows", ["Table Layout", "Rows & Columns"], None, "{row?, id?}", has_selection, |s, p| rows_cols(
            s,
            p,
            "delRow",
            "table.deleteRow"
        )),
        cmd!("table.deleteColumn", "Delete Columns", ["Table Layout", "Rows & Columns"], None, "{col?, id?}", has_selection, |s, p| rows_cols(
            s,
            p,
            "delCol",
            "table.deleteColumn"
        )),
        cmd!("table.merge", "Merge Cells", ["Table Layout", "Merge"], None, "{from: [r, c], to: [r, c], id?}", has_selection, merge),
        cmd!(
            "table.split",
            "Split Cells",
            ["Table Layout", "Merge"],
            None,
            "{cell?: [r, c] (else the edited cell), id?} | {row?: index, col?: index, id?}",
            has_selection,
            split
        ),
        cmd!(
            "table.style",
            "Table Styles",
            ["Table Design", "Table Styles"],
            None,
            "{style: medium2-accent1|light1-accent2|dark1-tx1|grid|none…, id?}",
            has_selection,
            style
        ),
        cmd!(
            "table.options",
            "Table Style Options",
            ["Table Design", "Table Style Options"],
            None,
            "{headerRow?: bool, totalRow?: bool, bandedRows?: bool, firstColumn?: bool, lastColumn?: bool, bandedColumns?: bool, id?}",
            has_selection,
            options
        ),
        cmd!(
            "table.cellFill",
            "Shading",
            ["Table Design", "Table Styles"],
            None,
            "{color? | none?, cells?: [[r,c],…] (default: selected cells or all), id?}",
            has_selection,
            cell_fill
        ),
        cmd!(
            "table.borders",
            "Borders",
            ["Table Design", "Draw Borders"],
            None,
            "{which: all|outside|inside|insideH|insideV|top|bottom|left|right|diagDown|diagUp|none, color?, width?: pt, dash?: solid|dash|dot…, cells?: [[r,c],…] (default: selected cells or all), id?}",
            has_selection,
            borders
        ),
        cmd!(
            "table.cellMargins",
            "Cell Margins",
            ["Table Layout", "Alignment"],
            None,
            "{preset?: normal|none|narrow|wide, left?, right?, top?, bottom?: pt, cells?, id?}",
            has_selection,
            cell_margins
        ),
        cmd!(
            "table.cellAlign",
            "Align Cell Text",
            ["Table Layout", "Alignment"],
            None,
            "{anchor?: top|middle|bottom (vertical; top when neither anchor nor align is given), align?: left|center|right|justify (horizontal, applied to every paragraph of the cells), cells?, id?}",
            has_selection,
            cell_align
        ),
        cmd!("table.distributeRows", "Distribute Rows", ["Table Layout", "Cell Size"], None, "{id?}", has_selection, |s, p| distribute(s, p, true)),
        cmd!("table.distributeColumns", "Distribute Columns", ["Table Layout", "Cell Size"], None, "{id?}", has_selection, |s, p| distribute(
            s, p, false
        )),
        cmd!(
            "table.columnWidth",
            "Width",
            ["Table Layout", "Cell Size"],
            None,
            "{col, width: pt, keepTotal?: bool (the next column absorbs the change), id?}",
            has_selection,
            column_width
        ),
        cmd!("table.rowHeight", "Height", ["Table Layout", "Cell Size"], None, "{row, height: pt, id?}", has_selection, row_height),
        cmd!(
            "table.drawLine",
            "Draw Table",
            ["Table Design", "Draw Borders"],
            None,
            "{from: [x, y], to: [x, y] slide pt, id?: table (default: the one under `from`)} splits the cells the line crosses",
            has_slide,
            draw_line
        ),
        cmd!(
            "table.eraseLine",
            "Table Eraser",
            ["Table Design", "Draw Borders"],
            None,
            "{at: [x, y] slide pt, id?, tol?: pt (default 6)} merges the two cells on either side of the nearest border",
            has_slide,
            erase_line
        ),
        cmd!(noundo "table.selectCells", "Select Cells", [], None, "{from: [r,c], to: [r,c], id?}", has_selection, select_cells),
    ]
}

/// Narrowest a column gets when its border is dragged (points).
pub const MIN_COLUMN_WIDTH: f64 = 12.0;

/// A table as drawn on the slide: where it is, its drawn row heights and its visible cells.
#[derive(Clone, Debug)]
pub struct TableFrame {
    /// Table-local (points from the table's top-left corner) → slide space, as the table is drawn.
    pub affine: Affine,
    pub cols: Vec<f64>,
    /// Row heights as drawn: a stored height is a minimum, rows grow to fit their text.
    pub heights: Vec<f64>,
    /// Visible cells (merges honoured): (row, col, table-local rect).
    pub cells: Vec<(usize, usize, Rect)>,
}

/// A border between table rows or columns that can be dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableBorder {
    /// The right edge of this column (the last one is the table's right edge).
    Column(usize),
    /// The bottom edge of this row (the last one is the table's bottom edge).
    Row(usize),
}

/// The drawn geometry of table shape `sh` on the edited slide (`None` if it isn't a table).
pub fn table_frame(st: &DocState, sh: &Shape) -> Option<TableFrame> {
    let ShapeKind::Table(t) = &sh.kind else { return None };
    // The table is drawn in its own box; rows that grow past it don't move it.
    let x = sh.xfrm.unwrap_or_else(|| xfrm_of(&st.doc, &st.selection, sh));
    let heights = table_drawn_heights(&st.doc, &st.selection, t);
    let cells = deckcraft_render::table_cell_rects(t, &heights);
    Some(TableFrame { affine: x.affine(), cols: t.cols.clone(), heights, cells })
}

impl TableFrame {
    pub fn width(&self) -> f64 {
        self.cols.iter().map(|w| w.max(0.0)).sum()
    }

    pub fn height(&self) -> f64 {
        self.heights.iter().map(|h| h.max(0.0)).sum()
    }

    /// Table-local point for slide point `p`.
    pub fn local(&self, p: Point) -> Point {
        self.affine.inverse() * p
    }

    /// Table-local rect of visible cell (`r`, `c`).
    pub fn cell_rect(&self, r: usize, c: usize) -> Option<Rect> {
        self.cells.iter().find(|(cr, cc, _)| (*cr, *cc) == (r, c)).map(|x| x.2)
    }

    /// The visible cell at table-local point `p` (points outside the table go to the nearest cell).
    pub fn cell_at(&self, p: Point) -> Option<(usize, usize)> {
        let q = Point::new(p.x.clamp(0.0, self.width().max(0.0)), p.y.clamp(0.0, self.height().max(0.0)));
        let inside = |r: &Rect| q.x >= r.x0 && q.x <= r.x1 && q.y >= r.y0 && q.y <= r.y1;
        // Edges are shared: prefer the cell the point is strictly inside.
        self.cells
            .iter()
            .find(|(_, _, r)| q.x >= r.x0 && q.x < r.x1 && q.y >= r.y0 && q.y < r.y1)
            .or_else(|| self.cells.iter().rev().find(|(_, _, r)| inside(r)))
            .map(|(r, c, _)| (*r, *c))
    }

    /// The draggable border within `tol` of table-local point `p`: the inner borders and the right
    /// and bottom edges (the left and top edges move the table). Borders hidden inside merged
    /// cells don't count.
    pub fn border_at(&self, p: Point, tol: f64) -> Option<TableBorder> {
        let (w, h) = (self.width(), self.height());
        if !(p.x >= -tol && p.y >= -tol && p.x <= w + tol && p.y <= h + tol) {
            return None;
        }
        let q = Point::new(p.x.clamp(0.0, w), p.y.clamp(0.0, h));
        let mut best: Option<(f64, TableBorder)> = None;
        let mut x = 0.0;
        for (i, cw) in self.cols.iter().enumerate() {
            x += cw.max(0.0);
            let d = (p.x - x).abs();
            // A cell ending at this edge at this height shows the border.
            let shown = self.cells.iter().any(|(_, _, r)| (r.x1 - x).abs() < 1e-6 && q.y >= r.y0 && q.y <= r.y1);
            if d <= tol && shown && best.is_none_or(|b| d < b.0) {
                best = Some((d, TableBorder::Column(i)));
            }
        }
        let mut y = 0.0;
        for (i, rh) in self.heights.iter().enumerate() {
            y += rh.max(0.0);
            let d = (p.y - y).abs();
            let shown = self.cells.iter().any(|(_, _, r)| (r.y1 - y).abs() < 1e-6 && q.x >= r.x0 && q.x <= r.x1);
            if d <= tol && shown && best.is_none_or(|b| d < b.0) {
                best = Some((d, TableBorder::Row(i)));
            }
        }
        best.map(|b| b.1)
    }
}

/// A `[r, c]` cell param: `Ok(None)` when absent (callers may fall back to the selection), `Err` when
/// present but not a valid cell, so a bad cell never acts on the selection.
fn cell_param(p: &Value, key: &str, cmd: &str) -> Result<Option<(usize, usize)>> {
    let v = match p.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    // Exactly [row, col]: an entry with more is as malformed as one with less.
    let idx = |x: &Value| x.as_u64().and_then(|x| usize::try_from(x).ok());
    match v.as_array().map(Vec::as_slice) {
        Some([r, c]) if let (Some(r), Some(c)) = (idx(r), idx(c)) => Ok(Some((r, c))),
        _ => Err(bad(cmd, format!("`{key}` is not a valid cell: {v}"))),
    }
}

/// The table's row/col to act on: params, the edited cell, or the selected cells.
/// A present but invalid `row`/`col` is an error; only an absent one falls back.
fn at(s: &Session, p: &Value, cmd: &str) -> Result<(usize, usize)> {
    let st = s.active();
    let cell = st.and_then(|d| d.selection.text.as_ref().and_then(|t| t.cell)).or_else(|| st.and_then(|d| d.selection.cells.map(|c| c.1)));
    Ok((opt_usize_param(p, "row", cmd)?.or(cell.map(|c| c.0)).unwrap_or(0), opt_usize_param(p, "col", cmd)?.or(cell.map(|c| c.1)).unwrap_or(0)))
}

fn table_id(s: &Session, p: &Value, cmd: &str) -> Result<deckcraft_model::ShapeId> {
    Ok(match id_param(p, "id", cmd)? {
        Some(i) => i,
        None => {
            let st = s.doc()?;
            st.selection
                .text
                .as_ref()
                .filter(|t| t.cell.is_some())
                .map(|t| t.shape)
                .or(st.selection.cells.map(|c| c.0))
                .or_else(|| st.selection.shapes.first().copied())
                .ok_or_else(|| bad(cmd, "select a table"))?
        }
    })
}

fn check_normalization(t: &Table, cmd: &str) -> Result<()> {
    if !t.can_normalize() {
        return Err(bad(cmd, "table cell limit or declared width exceeded; existing table data preserved"));
    }
    Ok(())
}

fn check_growth(t: &Table, rows: bool, cmd: &str) -> Result<()> {
    let fits = if rows {
        t.n_rows().checked_add(1).is_some_and(|n| Table::fits_cell_limit(n, t.n_cols()))
    } else {
        t.n_cols().checked_add(1).is_some_and(|n| Table::fits_cell_limit(t.n_rows(), n))
    };
    if !fits {
        return Err(bad(cmd, "table cell limit exceeded; existing table data preserved"));
    }
    Ok(())
}

/// Read-only admission for every affected table cell, merge-work, target-list and drawing
/// budget before an outer command settles an active composition.
pub(crate) fn preflight_table_budget(s: &Session, cmd: &str, p: &Value) -> Result<()> {
    if matches!(cmd, "table.cellFill" | "table.borders" | "table.cellMargins" | "table.cellAlign") {
        // target_cells executes finite range planning before it returns its target closure.
        let _targets = target_cells(s, p, cmd)?;
    }
    let mut draw = None;
    let id = match cmd {
        "table.drawLine" => {
            let (Some(from), Some(to)) = (point_param(p, "from"), point_param(p, "to")) else { return Ok(()) };
            let Some((id, origin)) = table_at(s, p, from) else { return Ok(()) };
            draw = Some(((from.0 - origin.0, from.1 - origin.1), (to.0 - origin.0, to.1 - origin.1)));
            id
        }
        "table.eraseLine" => {
            let Some(at) = point_param(p, "at") else { return Ok(()) };
            let Some((id, _)) = table_at(s, p, at) else { return Ok(()) };
            id
        }
        "edit.delete" => {
            let Some(st) = s.active() else { return Ok(()) };
            // While text is edited, Delete edits that text instead of clearing a cell range.
            if st.selection.text.is_some() || super::ids_param(p, "ids").is_some() || p.get("id").is_some() {
                return Ok(());
            }
            let Some((id, ..)) = st.selection.cells.filter(|c| st.selection.shapes == [c.0]) else { return Ok(()) };
            id
        }
        "table.insertRowAbove"
        | "table.insertRowBelow"
        | "table.insertColumnLeft"
        | "table.insertColumnRight"
        | "table.deleteRow"
        | "table.deleteColumn" => {
            if at(s, p, cmd).is_err() {
                return Ok(());
            }
            let Ok(id) = table_id(s, p, cmd) else { return Ok(()) };
            id
        }
        "table.merge"
        | "table.split"
        | "table.style"
        | "table.options"
        | "table.cellFill"
        | "table.borders"
        | "table.cellMargins"
        | "table.cellAlign"
        | "table.distributeRows"
        | "table.distributeColumns"
        | "table.columnWidth"
        | "table.rowHeight" => {
            let Ok(id) = table_id(s, p, cmd) else { return Ok(()) };
            id
        }
        _ => return Ok(()),
    };
    let Some(ShapeKind::Table(t)) = s.active().and_then(|st| st.shape(id)).map(|sh| &sh.kind) else { return Ok(()) };
    check_normalization(t, cmd)?;
    match cmd {
        "table.insertRowAbove" | "table.insertRowBelow" => check_growth(t, true, cmd)?,
        "table.insertColumnLeft" | "table.insertColumnRight" => check_growth(t, false, cmd)?,
        "table.merge" => {
            let (from, to) = merge_selection(s, p)?;
            if t.merge_bounds(from.0, from.1, to.0, to.1).is_none() {
                return Err(bad(cmd, "can't merge those cells"));
            }
        }
        _ => {}
    }
    if let Some((from, to)) = draw {
        check_draw_admission(t, from, to, cmd)?;
    }
    Ok(())
}

/// Geometry-only copy for the real pencil algorithm: never copy text, formatting or history.
fn budget_grid(t: &Table) -> Table {
    Table {
        cols: t.cols.clone(),
        rows: t
            .rows
            .iter()
            .map(|row| deckcraft_model::table::Row {
                height: row.height,
                cells: row
                    .cells
                    .iter()
                    .map(|cell| deckcraft_model::table::Cell {
                        grid_span: cell.grid_span,
                        row_span: cell.row_span,
                        h_merge: cell.h_merge,
                        v_merge: cell.v_merge,
                        ..Default::default()
                    })
                    .collect(),
            })
            .collect(),
        ..Default::default()
    }
}

fn check_draw_admission(t: &Table, from: (f64, f64), to: (f64, f64), cmd: &str) -> Result<()> {
    check_normalization(t, cmd)?;
    let mut preview = budget_grid(t);
    preview.normalize();
    // The same helper decides geometry and its own MAX_GRID refusal. This also keeps
    // no-change pencil errors before settlement, without copying any cell text.
    if !crate::drawtable::draw_line(&mut preview, from, to) {
        return Err(bad(cmd, "the line doesn't divide any cell"));
    }
    check_normalization(&preview, cmd)
}

fn with_table(s: &mut Session, p: &Value, cmd: &str, f: impl Fn(&mut Table) -> Result<()>) -> Result<Value> {
    let id = table_id(s, p, cmd)?;
    // Refuse unsupported normalization before edit_shapes clones or measures the table.
    if let Some(ShapeKind::Table(t)) = s.doc()?.shape(id).map(|sh| &sh.kind) {
        check_normalization(t, cmd)?;
    }
    let r = edit_shapes(s, &json!({"id": id}), cmd, |sh| match &mut sh.kind {
        ShapeKind::Table(t) => {
            // Missing cells exist before targeting. Refusal never truncates the original table.
            if !t.try_normalize() {
                return Err(bad(cmd, "table cell limit or declared width exceeded; existing table data preserved"));
            }
            f(t)?;
            // A callback can grow the grid (including draw/erase helpers). Session::edit does
            // not commit this copy when normalization refuses, so all original data survives.
            if !t.try_normalize() {
                return Err(bad(cmd, "table cell limit or declared width exceeded; existing table data preserved"));
            }
            let (w, h) = (t.width(), t.height());
            if let Some(x) = sh.xfrm.as_mut() {
                x.w = w;
                x.h = h;
            }
            Ok(())
        }
        _ => Err(bad(cmd, "not a table")),
    })?;
    // The edited cell may have moved out of range.
    s.select(|doc, sel| {
        let dims = super::current_shapes(doc, sel)
            .and_then(|v| deckcraft_model::find_shape(v, id))
            .and_then(|x| if let ShapeKind::Table(t) = &x.kind { Some((t.n_rows(), t.n_cols())) } else { None });
        if let (Some(t), Some((nr, nc))) = (sel.text.as_mut(), dims)
            && let Some((r, c)) = t.cell
            && (r >= nr || c >= nc)
        {
            t.cell = Some((r.min(nr.saturating_sub(1)), c.min(nc.saturating_sub(1))));
            t.anchor = (0, 0);
            t.caret = (0, 0);
        }
    })?;
    Ok(r)
}

fn rows_cols(s: &mut Session, p: &Value, op: &str, name: &'static str) -> Result<Value> {
    let (r, c) = at(s, p, name)?;
    let op = op.to_string();
    with_table(s, p, name, move |t| {
        match op.as_str() {
            "rowAbove" | "rowBelow" => check_growth(t, true, name)?,
            "colLeft" | "colRight" => check_growth(t, false, name)?,
            _ => {}
        }
        match op.as_str() {
            "rowAbove" => t.insert_row(r),
            "rowBelow" => t.insert_row(r.saturating_add(1)),
            "colLeft" => t.insert_col(c),
            "colRight" => t.insert_col(c.saturating_add(1)),
            "delRow" => {
                if !t.delete_row(r) {
                    return Err(bad(name, "can't delete the last row"));
                }
            }
            _ => {
                if !t.delete_col(c) {
                    return Err(bad(name, "can't delete the last column"));
                }
            }
        }
        Ok(())
    })
}

fn merge_selection(s: &Session, p: &Value) -> Result<((usize, usize), (usize, usize))> {
    let sel = s.doc()?.selection.cells;
    let from = cell_param(p, "from", "table.merge")?.or(sel.map(|c| c.1)).ok_or_else(|| bad("table.merge", "missing `from`"))?;
    let to = cell_param(p, "to", "table.merge")?.or(sel.map(|c| c.2)).ok_or_else(|| bad("table.merge", "missing `to`"))?;
    Ok((from, to))
}

fn merge(s: &mut Session, p: &Value) -> Result<Value> {
    let (from, to) = merge_selection(s, p)?;
    with_table(
        s,
        p,
        "table.merge",
        move |t| if t.merge(from.0, from.1, to.0, to.1) { Ok(()) } else { Err(bad("table.merge", "can't merge those cells")) },
    )
}

fn split(s: &mut Session, p: &Value) -> Result<Value> {
    let cell = match cell_param(p, "cell", "table.split")? {
        Some(c) => c,
        None => at(s, p, "table.split")?,
    };
    with_table(s, p, "table.split", move |t| if t.split(cell.0, cell.1) { Ok(()) } else { Err(bad("table.split", "that cell isn't merged")) })
}

fn style(s: &mut Session, p: &Value) -> Result<Value> {
    let st = str_param(p, "style").ok_or_else(|| bad("table.style", "missing `style`"))?.to_string();
    with_table(s, p, "table.style", move |t| {
        t.style = st.clone();
        Ok(())
    })
}

fn options(s: &mut Session, p: &Value) -> Result<Value> {
    with_table(s, p, "table.options", |t| {
        if let Some(v) = bool_param(p, "headerRow") {
            t.first_row = v;
        }
        if let Some(v) = bool_param(p, "totalRow") {
            t.last_row = v;
        }
        if let Some(v) = bool_param(p, "bandedRows") {
            t.band_row = v;
        }
        if let Some(v) = bool_param(p, "firstColumn") {
            t.first_col = v;
        }
        if let Some(v) = bool_param(p, "lastColumn") {
            t.last_col = v;
        }
        if let Some(v) = bool_param(p, "bandedColumns") {
            t.band_col = v;
        }
        Ok(())
    })
}

/// At most this many `cells` in one table cell command.
const MAX_CELLS: usize = 100_000;

/// At most 1,600,000 normalized grid-slot visits per formatting expansion. A composing
/// command plans once in outer admission and once in its handler, at most twice this work;
/// the returned target closure reuses the rectangle instead of expanding a third time.
const MAX_FORMAT_WORK: usize = 16 * MAX_CELLS;

/// Formatting-only legacy range expansion with finite work, before cloning or normalization.
/// Missing cells would normalize to uncovered unit cells, which cannot grow the rectangle.
/// Charge their slots too so sparse preflight and the later dense edit have identical budgets.
fn formatting_cell_block(t: &Table, a: (usize, usize), b: (usize, usize), cmd: &str) -> Result<((usize, usize), (usize, usize))> {
    check_normalization(t, cmd)?;
    let refused = || bad(cmd, format!("selected cell expansion work limit exceeded; at most {MAX_FORMAT_WORK} cell visits per expansion"));
    let scan = t.n_rows().checked_mul(t.n_cols()).ok_or_else(refused)?;
    let (nr, nc) = (t.n_rows().max(1), t.n_cols().max(1));
    let (a, b) = ((a.0.min(nr - 1), a.1.min(nc - 1)), (b.0.min(nr - 1), b.1.min(nc - 1)));
    let (mut r0, mut r1, mut c0, mut c1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
    let mut work = 0usize;
    for _ in 0..nr + nc {
        // Reserve the complete scan, including its final stable pass; never admit a prefix.
        work = work.checked_add(scan).filter(|work| *work <= MAX_FORMAT_WORK).ok_or_else(refused)?;
        let mut grown = false;
        for (r, row) in t.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                if cell.is_covered() {
                    continue;
                }
                let end = |at: usize, span: u32, n: usize| at.saturating_add((span.max(1) - 1) as usize).min(n - 1);
                let (er, ec) = (end(r, cell.row_span, nr), end(c, cell.grid_span, nc));
                let overlaps = r <= r1 && er >= r0 && c <= c1 && ec >= c0;
                if overlaps && (r < r0 || er > r1 || c < c0 || ec > c1) {
                    (r0, r1, c0, c1) = (r0.min(r), r1.max(er), c0.min(c), c1.max(ec));
                    grown = true;
                }
            }
        }
        if !grown {
            break;
        }
    }
    let pick = |lo: usize, hi: usize, first: bool| if first { lo } else { hi };
    let from = (pick(r0, r1, a.0 <= b.0), pick(c0, c1, a.1 <= b.1));
    let to = (pick(r0, r1, a.0 > b.0), pick(c0, c1, a.1 > b.1));
    Ok((from, to))
}

/// The cells a command acts on: `cells`, else the selected cell range, else the cell holding the
/// caret, else every cell.
fn target_cells(s: &Session, p: &Value, cmd: &str) -> Result<impl Fn(&Table) -> Result<Vec<(usize, usize)>> + use<>> {
    // `cells` is a list of [row, col]: anything else, or an entry that is not one, is refused, never
    // dropped or read as absent (which would act on the selection or the whole table).
    let cells: Option<Vec<(usize, usize)>> = match p.get("cells") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if a.len() > MAX_CELLS => {
            return Err(bad(cmd, format!("`cells` lists {} cells; at most {MAX_CELLS}", a.len())));
        }
        Some(Value::Array(a)) => Some(
            a.iter()
                .map(|c| {
                    cell_param(&json!({ "cell": c }), "cell", cmd)?.ok_or_else(|| bad(cmd, format!("`cells` entry {c} is not a [row, col] cell")))
                })
                .collect::<Result<_>>()?,
        ),
        Some(v) => return Err(bad(cmd, format!("`cells` is a list of [row, col] cells, not {v}"))),
    };
    let st = s.doc()?;
    // A selected range grows over the merged cells it touches; with only a caret, the caret's cell is used.
    let sel = st.selection.cells.map(|(_, a, b)| (a, b, true)).or_else(|| st.selection.text.as_ref().and_then(|t| t.cell).map(|c| (c, c, false)));
    // Execute finite selected-range admission now, before returning a closure and before
    // the handler can clone the document. The actual target uses the same table_id lookup.
    let planned_range = if cells.is_none()
        && let Some((a, b, true)) = sel
    {
        let table = table_id(s, p, cmd).ok().and_then(|id| st.shape(id)).and_then(|sh| match &sh.kind {
            ShapeKind::Table(t) => Some(t),
            _ => None,
        });
        table.map(|t| formatting_cell_block(t, a, b, cmd)).transpose()?
    } else {
        None
    };
    let cmd = cmd.to_string();
    Ok(move |t: &Table| match (&cells, sel) {
        // Each listed cell must be in the table, so a command never reports success for a cell
        // it could not edit.
        (Some(c), _) => match c.iter().find(|(r, c)| *r >= t.n_rows() || *c >= t.n_cols()) {
            Some((r, c)) => Err(bad(&cmd, format!("no cell [{r}, {c}]: the table has {} rows and {} columns", t.n_rows(), t.n_cols()))),
            None => Ok(c.clone()),
        },
        (None, Some((a, b, range))) => {
            let (a, b) = if range { planned_range.map(Ok).unwrap_or_else(|| formatting_cell_block(t, a, b, &cmd))? } else { (a, b) };
            // The selection can name cells outside the table; only walk the ones that exist.
            let (r1, c1) = (a.0.max(b.0).min(t.n_rows().saturating_sub(1)), a.1.max(b.1).min(t.n_cols().saturating_sub(1)));
            Ok((a.0.min(b.0)..=r1).flat_map(|r| (a.1.min(b.1)..=c1).map(move |c| (r, c))).collect())
        }
        _ => Ok((0..t.n_rows()).flat_map(|r| (0..t.n_cols()).map(move |c| (r, c))).collect()),
    })
}

fn cell_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let fill = if bool_or(p, "none", false) { Some(deckcraft_model::Fill::None) } else { color_param(p, "color").map(deckcraft_model::Fill::solid) };
    let targets = target_cells(s, p, "table.cellFill")?;
    with_table(s, p, "table.cellFill", move |t| {
        for (r, c) in targets(t)? {
            if let Some(cell) = t.cell_mut(r, c) {
                cell.fill.clone_from(&fill);
            }
        }
        Ok(())
    })
}

const LEFT: usize = 0;
const RIGHT: usize = 1;
const TOP: usize = 2;
const BOTTOM: usize = 3;

fn borders(s: &mut Session, p: &Value) -> Result<Value> {
    use deckcraft_model::Line;
    let which = str_param(p, "which").unwrap_or("all").to_string();
    const WHICH: [&str; 12] = ["all", "outside", "inside", "insideH", "insideV", "top", "bottom", "left", "right", "diagDown", "diagUp", "none"];
    if !WHICH.contains(&which.as_str()) {
        return Err(bad("table.borders", format!("unknown `which` {which:?}; one of {}", WHICH.join(", "))));
    }
    let color = color_param(p, "color").unwrap_or_else(|| deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0)));
    let width = f64_param(p, "width").filter(|w| w.is_finite()).unwrap_or(1.0).clamp(0.0, 100.0);
    let mut line = Line::solid(color, width);
    if let Some(d) = str_param(p, "dash").filter(|d| *d != "solid") {
        if !deckcraft_model::style::Dash::PRESETS.contains(&d) {
            return Err(bad("table.borders", format!("unknown dash {d:?}; one of {}", deckcraft_model::style::Dash::PRESETS.join(", "))));
        }
        line.dash = Some(deckcraft_model::style::Dash::Preset { name: d.to_string() });
    }
    // "none" clears to no line, so the table style's own lines don't show through either.
    let line = if which == "none" { Line::none() } else { line };
    let targets = target_cells(s, p, "table.borders")?;
    with_table(s, p, "table.borders", move |t| {
        let list = targets(t)?;
        let (Some(r0), Some(r1), Some(c0), Some(c1)) =
            (list.iter().map(|x| x.0).min(), list.iter().map(|x| x.0).max(), list.iter().map(|x| x.1).min(), list.iter().map(|x| x.1).max())
        else {
            return Ok(());
        };
        let (rows, cols) = (t.n_rows(), t.n_cols());
        // Set one side of a cell and the facing side of its neighbour, as PowerPoint does.
        let set = |t: &mut Table, r: usize, c: usize, side: usize| {
            if let Some(cell) = t.cell_mut(r, c) {
                cell.borders[side] = Some(line.clone());
            }
            let n = match side {
                LEFT => c.checked_sub(1).map(|c| (r, c, RIGHT)),
                RIGHT => c.checked_add(1).filter(|c| *c < cols).map(|c| (r, c, LEFT)),
                TOP => r.checked_sub(1).map(|r| (r, c, BOTTOM)),
                _ => r.checked_add(1).filter(|r| *r < rows).map(|r| (r, c, TOP)),
            };
            if let Some((r, c, side)) = n
                && let Some(cell) = t.cell_mut(r, c)
            {
                cell.borders[side] = Some(line.clone());
            }
        };
        for (r, c) in list {
            if r < r0 || r > r1 || c < c0 || c > c1 {
                continue;
            }
            let outer = [c == c0, c == c1, r == r0, r == r1];
            for side in [LEFT, RIGHT, TOP, BOTTOM] {
                let horizontal = side == TOP || side == BOTTOM;
                let on = match which.as_str() {
                    "all" | "none" => true,
                    "outside" => outer[side],
                    "inside" => !outer[side],
                    "insideH" => horizontal && !outer[side],
                    "insideV" => !horizontal && !outer[side],
                    "top" => side == TOP && outer[side],
                    "bottom" => side == BOTTOM && outer[side],
                    "left" => side == LEFT && outer[side],
                    "right" => side == RIGHT && outer[side],
                    _ => false,
                };
                if on {
                    set(t, r, c, side);
                }
            }
            if let Some(cell) = t.cell_mut(r, c) {
                match which.as_str() {
                    "diagDown" => cell.diag_down = Some(line.clone()),
                    "diagUp" => cell.diag_up = Some(line.clone()),
                    "none" => {
                        cell.diag_down = None;
                        cell.diag_up = None;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })
}

fn cell_margins(s: &mut Session, p: &Value) -> Result<Value> {
    // PowerPoint's presets (left/right, top/bottom).
    let base = match str_param(p, "preset") {
        Some("none") => [0.0; 4],
        Some("narrow") => [3.6; 4],
        Some("wide") => [14.4, 14.4, 14.4, 14.4],
        Some("normal") | None => [7.2, 7.2, 3.6, 3.6],
        Some(o) => return Err(bad("table.cellMargins", format!("unknown preset {o:?}; one of normal, none, narrow, wide"))),
    };
    let side = |k: &str, i: usize| f64_param(p, k).filter(|v| v.is_finite()).map_or(base[i], |v| v.clamp(0.0, 400.0));
    let m = [side("left", 0), side("right", 1), side("top", 2), side("bottom", 3)];
    let targets = target_cells(s, p, "table.cellMargins")?;
    with_table(s, p, "table.cellMargins", move |t| {
        for (r, c) in targets(t)? {
            if let Some(cell) = t.cell_mut(r, c) {
                cell.margins = Some(m);
            }
        }
        Ok(())
    })
}

fn cell_align(s: &mut Session, p: &Value) -> Result<Value> {
    use deckcraft_model::text::{Align, Anchor};
    // A value that is not a string is refused, not taken as absent.
    for k in ["align", "anchor"] {
        if p.get(k).is_some_and(|v| !v.is_null() && !v.is_string()) {
            return Err(bad("table.cellAlign", format!("`{k}` is a name such as {}", if k == "align" { "left or center" } else { "top or middle" })));
        }
    }
    // Without `align`, the vertical anchor is the whole command (top when not given).
    let align = match str_param(p, "align") {
        None => None,
        Some("left" | "l") => Some(Align::Left),
        Some("center" | "centre" | "ctr") => Some(Align::Center),
        Some("right" | "r") => Some(Align::Right),
        Some("justify" | "just") => Some(Align::Justify),
        Some(o) => return Err(bad("table.cellAlign", format!("unknown align {o:?}; one of left, center, right, justify"))),
    };
    let anchor = match (str_param(p, "anchor"), align) {
        (None, Some(_)) => None,
        (a, _) => Some(match a.unwrap_or("top") {
            "middle" | "center" | "ctr" => Anchor::Middle,
            "bottom" | "b" => Anchor::Bottom,
            "top" | "t" => Anchor::Top,
            o => return Err(bad("table.cellAlign", format!("unknown anchor {o:?}; one of top, middle, bottom"))),
        }),
    };
    let targets = target_cells(s, p, "table.cellAlign")?;
    with_table(s, p, "table.cellAlign", move |t| {
        for (r, c) in targets(t)? {
            if let Some(cell) = t.cell_mut(r, c) {
                if anchor.is_some() {
                    cell.anchor = anchor;
                }
                if let Some(a) = align {
                    for para in &mut cell.text.paragraphs {
                        para.props.align = Some(a);
                    }
                }
            }
        }
        Ok(())
    })
}

fn distribute(s: &mut Session, p: &Value, rows: bool) -> Result<Value> {
    with_table(s, p, "table.distribute", move |t| {
        if rows {
            let h = t.height() / t.n_rows().max(1) as f64;
            t.rows.iter_mut().for_each(|r| r.height = h);
        } else {
            let w = t.width() / t.n_cols().max(1) as f64;
            t.cols.iter_mut().for_each(|c| *c = w);
        }
        Ok(())
    })
}

fn column_width(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, c) = at(s, p, "table.columnWidth")?;
    let w = f64_param(p, "width").ok_or_else(|| bad("table.columnWidth", "missing `width`"))?.clamp(4.0, 4000.0);
    let keep_total = bool_or(p, "keepTotal", false);
    with_table(s, p, "table.columnWidth", move |t| {
        // Keeping the table's width, the column to the right gives or takes the difference (dragging
        // an inner border). The last column has none: the table gets wider or narrower.
        let pair = t.cols.get(c).zip(t.cols.get(c.saturating_add(1))).map(|(a, b)| a.max(0.0) + b.max(0.0)).filter(|x| x.is_finite());
        match pair {
            Some(pair) if keep_total => {
                let min = MIN_COLUMN_WIDTH.min(pair / 2.0);
                let w = w.max(min).min(pair - min);
                if let Some(x) = t.cols.get_mut(c) {
                    *x = w;
                }
                if let Some(x) = t.cols.get_mut(c.saturating_add(1)) {
                    *x = pair - w;
                }
            }
            _ => {
                if let Some(x) = t.cols.get_mut(c) {
                    *x = w;
                }
            }
        }
        Ok(())
    })
}

fn row_height(s: &mut Session, p: &Value) -> Result<Value> {
    let (r, _) = at(s, p, "table.rowHeight")?;
    let h = f64_param(p, "height").ok_or_else(|| bad("table.rowHeight", "missing `height`"))?.clamp(4.0, 4000.0);
    with_table(s, p, "table.rowHeight", move |t| {
        if let Some(x) = t.rows.get_mut(r) {
            x.height = h;
        }
        Ok(())
    })
}

fn select_cells(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match id_param(p, "id", "table.selectCells")? {
        Some(i) => i,
        None => s.doc()?.selection.shapes.first().copied().ok_or_else(|| bad("table.selectCells", "select a table"))?,
    };
    let from = cell_param(p, "from", "table.selectCells")?.unwrap_or((0, 0));
    let to = cell_param(p, "to", "table.selectCells")?.unwrap_or(from);
    s.select(|_, sel| {
        sel.cells = Some((id, from, to));
        sel.shapes = vec![id];
        sel.text = None;
    })?;
    ok()
}

/// The cell range from `a` to `b` in `t`, grown until every merged cell it touches lies inside
/// it, as (the corner on `a`'s side, the corner on `b`'s side). Cells past the table's edge are
/// clamped to it.
pub fn cell_block(t: &Table, a: (usize, usize), b: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    let (nr, nc) = (t.n_rows().max(1), t.n_cols().max(1));
    let (a, b) = ((a.0.min(nr - 1), a.1.min(nc - 1)), (b.0.min(nr - 1), b.1.min(nc - 1)));
    let (mut r0, mut r1, mut c0, mut c1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
    // Each pass takes in the merged cells that overlap the block; stop when one adds nothing.
    for _ in 0..nr + nc {
        let mut grown = false;
        for (r, row) in t.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                if cell.is_covered() {
                    continue;
                }
                // Spans come from the file; the additions saturate so that they cannot overflow on 32-bit targets.
                let end = |at: usize, span: u32, n: usize| at.saturating_add((span.max(1) - 1) as usize).min(n - 1);
                let (er, ec) = (end(r, cell.row_span, nr), end(c, cell.grid_span, nc));
                let overlaps = r <= r1 && er >= r0 && c <= c1 && ec >= c0;
                if overlaps && (r < r0 || er > r1 || c < c0 || ec > c1) {
                    (r0, r1, c0, c1) = (r0.min(r), r1.max(er), c0.min(c), c1.max(ec));
                    grown = true;
                }
            }
        }
        if !grown {
            break;
        }
    }
    let pick = |lo: usize, hi: usize, first: bool| if first { lo } else { hi };
    let from = (pick(r0, r1, a.0 <= b.0), pick(c0, c1, a.1 <= b.1));
    let to = (pick(r0, r1, a.0 > b.0), pick(c0, c1, a.1 > b.1));
    (from, to)
}

/// The selected cell range of table `id` (`t`), grown over merged cells, as its top-left and
/// bottom-right cells. `None` when no range of that table is selected.
pub fn selected_range(
    cells: Option<(deckcraft_model::ShapeId, (usize, usize), (usize, usize))>,
    id: deckcraft_model::ShapeId,
    t: &Table,
) -> Option<((usize, usize), (usize, usize))> {
    let (_, a, b) = cells.filter(|c| c.0 == id)?;
    let (x, y) = cell_block(t, a, b);
    Some(((x.0.min(y.0), x.1.min(y.1)), (x.0.max(y.0), x.1.max(y.1))))
}

/// Whether cell (`r`, `c`) lies in `range` (from [`selected_range`]); without a range every cell does.
pub fn in_range(range: Option<((usize, usize), (usize, usize))>, r: usize, c: usize) -> bool {
    range.is_none_or(|((r0, c0), (r1, c1))| (r0..=r1).contains(&r) && (c0..=c1).contains(&c))
}

/// Shift+arrow with a cell range selected: the cell the range was extended to moves by (`dr`,
/// `dc`), on past a merged cell until the range changes. The cell it started from stays. Returns
/// false when no cell range of the selected table is selected.
pub(crate) fn extend_selected_cells(s: &mut Session, dr: isize, dc: isize) -> Result<bool> {
    let st = s.doc()?;
    let Some((id, from, to)) = st.selection.cells.filter(|c| st.selection.shapes == [c.0]) else { return Ok(false) };
    let Some(ShapeKind::Table(t)) = st.shape(id).map(|sh| &sh.kind) else { return Ok(false) };
    let (nr, nc) = (t.n_rows(), t.n_cols());
    let old = cell_block(t, from, to);
    let mut end = to;
    while cell_block(t, from, end) == old {
        let r = end.0.checked_add_signed(dr).filter(|r| *r < nr);
        let c = end.1.checked_add_signed(dc).filter(|c| *c < nc);
        let (Some(r), Some(c)) = (r, c) else { break };
        end = (r, c);
    }
    s.select(|_, sel| sel.cells = Some((id, from, end)))?;
    Ok(true)
}

/// The text of the selected range of cells, for Copy and Cut: a tab between the cells of a row
/// and a line per row (a merged cell once, where it starts). `None` without a cell range of the
/// selected table.
pub(crate) fn selected_cells_text(st: &DocState) -> Option<String> {
    let id = st.selection.cells.filter(|c| st.selection.shapes == [c.0])?.0;
    let ShapeKind::Table(t) = &st.shape(id)?.kind else { return None };
    let ((r0, c0), (r1, c1)) = selected_range(st.selection.cells, id, t)?;
    let rows = (r0..=r1).map(|r| {
        let cells = (c0..=c1).filter_map(|c| t.cell(r, c).filter(|x| !x.is_covered()).map(|x| x.text.text()));
        cells.collect::<Vec<_>>().join("\t")
    });
    Some(rows.collect::<Vec<_>>().join("\n"))
}

/// Delete with a cell range selected: the text in those cells goes and the cells stay, each
/// keeping its first paragraph's formatting.
pub(crate) fn clear_selected_cells(s: &mut Session) -> Result<Value> {
    let Some((id, ..)) = s.doc()?.selection.cells else { return ok() };
    let cells = s.doc()?.selection.cells;
    with_table(s, &json!({"id": id}), "edit.delete", move |t| {
        let range = selected_range(cells, id, t);
        for (r, row) in t.rows.iter_mut().enumerate() {
            for (c, cell) in row.cells.iter_mut().enumerate() {
                if in_range(range, r, c) {
                    deckcraft_model::edit::delete(&mut cell.text, (0, 0), (usize::MAX, usize::MAX));
                }
            }
        }
        Ok(())
    })
}

fn point_param(p: &Value, key: &str) -> Option<(f64, f64)> {
    let a = p.get(key)?;
    let (x, y) = (a.get(0)?.as_f64()?, a.get(1)?.as_f64()?);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// The table under slide point `at` (frontmost), or the one named by `id`, with its origin.
pub(crate) fn table_at(s: &Session, p: &Value, at: (f64, f64)) -> Option<(deckcraft_model::ShapeId, (f64, f64))> {
    let st = s.doc().ok()?;
    // A malformed id names no table (never the one under the pointer instead).
    let want = id_param(p, "id", "table.draw").ok()?;
    st.shapes().iter().rev().find_map(|sh| {
        let x = sh.xfrm?;
        let ShapeKind::Table(t) = &sh.kind else { return None };
        let inside = at.0 >= x.x && at.1 >= x.y && at.0 <= x.x + t.width() && at.1 <= x.y + t.height();
        (want.map_or(inside, |w| w == sh.id)).then_some((sh.id, (x.x, x.y)))
    })
}

fn draw_line(s: &mut Session, p: &Value) -> Result<Value> {
    let from = point_param(p, "from").ok_or_else(|| bad("table.drawLine", "missing `from`"))?;
    let to = point_param(p, "to").ok_or_else(|| bad("table.drawLine", "missing `to`"))?;
    let (id, o) = table_at(s, p, from).ok_or_else(|| bad("table.drawLine", "no table there"))?;
    let (a, b) = ((from.0 - o.0, from.1 - o.1), (to.0 - o.0, to.1 - o.1));
    if let Some(ShapeKind::Table(t)) = s.doc()?.shape(id).map(|sh| &sh.kind) {
        check_draw_admission(t, a, b, "table.drawLine")?;
    }
    with_table(s, &json!({"id": id.0}), "table.drawLine", move |t| {
        if crate::drawtable::draw_line(t, a, b) { Ok(()) } else { Err(bad("table.drawLine", "the line doesn't divide any cell")) }
    })
}

fn erase_line(s: &mut Session, p: &Value) -> Result<Value> {
    let at = point_param(p, "at").ok_or_else(|| bad("table.eraseLine", "missing `at`"))?;
    let tol = p.get("tol").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(6.0).clamp(0.5, 50.0);
    let (id, o) = table_at(s, p, at).ok_or_else(|| bad("table.eraseLine", "no table there"))?;
    let q = (at.0 - o.0, at.1 - o.1);
    with_table(s, &json!({"id": id.0}), "table.eraseLine", move |t| {
        if crate::drawtable::erase_at(t, q, tol) { Ok(()) } else { Err(bad("table.eraseLine", "no border to erase there")) }
    })
}

#[cfg(test)]
mod bounded_table_tests {
    use super::*;
    use deckcraft_model::{ShapeId, table::MAX_CELLS, table::Row};

    fn loaded_table(t: Table) -> (Session, ShapeId) {
        let mut s = Session::with_new();
        let id = ShapeId(s.execute("insert.table", &json!({"rows": 2, "cols": 2})).unwrap()["id"].as_u64().unwrap() as u32);
        s.edit(|doc, sel| {
            let shapes = crate::shapes_mut(doc, sel).unwrap();
            deckcraft_model::find_shape_mut(shapes, id).unwrap().kind = ShapeKind::Table(t);
            Ok(())
        })
        .unwrap();
        (s, id)
    }

    fn table(s: &Session, id: ShapeId) -> &Table {
        let ShapeKind::Table(t) = &s.doc().unwrap().shape(id).unwrap().kind else { panic!("not a table") };
        t
    }

    #[test]
    fn table_command_normalizes_loaded_sparse_grid_before_edit_and_undo_restores_it() {
        let sparse = Table { cols: vec![10.0; 1000], rows: vec![Row::default(); 100], ..Default::default() };
        let (mut s, id) = loaded_table(sparse.clone());
        s.execute("table.cellFill", &json!({"id": id.0, "cells": [[0, 999]], "color": "#FF0000"})).unwrap();
        let t = table(&s, id);
        assert_eq!((t.n_rows(), t.n_cols()), (MAX_CELLS / 1000, 1000));
        assert!(t.is_normalized());
        assert!(t.cell(0, 999).unwrap().fill.is_some(), "the command must edit a cell created before its callback");
        assert!(t.cell(0, 998).unwrap().fill.is_none());
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(&s, id), &sparse);
    }

    #[test]
    fn table_merge_work_refusal_rolls_back_document_selection_and_history() {
        let mut chain = Table::new(1000, 10, 400.0, 30.0);
        assert_eq!((chain.n_rows(), chain.n_cols()), (1000, 10));
        for row in &mut chain.rows {
            for cell in &mut row.cells {
                cell.row_span = 2;
            }
        }
        let (mut s, id) = loaded_table(chain.clone());
        let before_doc = s.doc().unwrap().doc.clone();
        let before_selection = serde_json::to_value(&s.doc().unwrap().selection).unwrap();
        let before_undo = s.doc().unwrap().history.undo.len();
        assert!(s.execute("table.merge", &json!({"id": id.0, "from": [998, 0], "to": [999, 9]})).is_err());
        assert_eq!(s.doc().unwrap().doc, before_doc);
        assert_eq!(table(&s, id), &chain);
        assert_eq!(serde_json::to_value(&s.doc().unwrap().selection).unwrap(), before_selection);
        assert_eq!(s.doc().unwrap().history.undo.len(), before_undo);
    }

    #[test]
    fn table_measurement_keeps_unsupported_grid_without_mutating_loaded_table() {
        let sparse = Table { cols: vec![10.0; 1000], rows: vec![Row { height: 30.0, ..Default::default() }; 101], ..Default::default() };
        let (s, id) = loaded_table(sparse.clone());
        let st = s.doc().unwrap();
        let shape = st.shape(id).unwrap();
        let before = shape.clone();
        let mut normalized = sparse;
        normalized.normalize();
        let drawn: f64 = table_drawn_heights(&st.doc, &st.selection, &normalized).iter().sum();
        let actual = xfrm_of(&st.doc, &st.selection, shape);
        assert_eq!(actual.h, shape.xfrm.unwrap().h.max(drawn));
        assert_eq!(st.shape(id).unwrap(), &before);
    }
}

#[cfg(test)]
mod retention_repair_tests {
    use super::*;
    use deckcraft_model::{
        ShapeId,
        table::{Cell, Row},
    };

    fn populated(rows: usize, cols: usize) -> Table {
        let mut t = Table::new(rows, cols, 400.0, 30.0);
        assert_eq!((t.n_rows(), t.n_cols()), (rows, cols));
        for (r, row) in t.rows.iter_mut().enumerate() {
            row.cells[0].text = deckcraft_model::TextBody::from_text(&format!("FIRST-{r}"));
            row.cells[cols - 1].text = deckcraft_model::TextBody::from_text(&format!("LAST-{r}"));
        }
        t
    }

    fn loaded(t: Table) -> (Session, ShapeId) {
        let mut s = Session::with_new();
        let id = ShapeId(s.execute("insert.table", &json!({"rows": 2, "cols": 2})).unwrap()["id"].as_u64().unwrap() as u32);
        s.edit(|doc, sel| {
            deckcraft_model::find_shape_mut(crate::shapes_mut(doc, sel).unwrap(), id).unwrap().kind = ShapeKind::Table(t);
            Ok(())
        })
        .unwrap();
        (s, id)
    }

    fn table(s: &Session, id: ShapeId) -> &Table {
        let ShapeKind::Table(t) = &s.doc().unwrap().shape(id).unwrap().kind else { panic!("not a table") };
        t
    }

    fn first_table(s: &Session) -> &Table {
        s.doc()
            .unwrap()
            .doc
            .slides
            .iter()
            .flat_map(|slide| &slide.shapes)
            .find_map(|shape| if let ShapeKind::Table(t) = &shape.kind { Some(t) } else { None })
            .unwrap()
    }

    fn history(
        entries: &[crate::HistoryEntry],
    ) -> Vec<(String, std::sync::Arc<deckcraft_model::Presentation>, crate::Selection, Option<(ShapeId, usize, usize, String)>)> {
        entries.iter().map(|e| (e.label.clone(), e.doc.clone(), e.selection.clone(), e.vertex_kind.clone())).collect()
    }

    fn refuse_without_change(s: &mut Session, command: &str, params: Value) {
        let st = s.doc().unwrap();
        let before = st.doc.clone();
        let selection = st.selection.clone();
        let undo = history(&st.history.undo);
        let redo = history(&st.history.redo);
        let costs = st.history.costs.clone();
        let limits = (st.history.limit, st.history.byte_limit);
        let revision = st.revision;
        let error = s.execute(command, &params).unwrap_err().to_string();
        assert!(error.contains(command) && error.contains("cell limit"), "named refusal required: {error}");
        let st = s.doc().unwrap();
        assert!(std::sync::Arc::ptr_eq(&st.doc, &before), "a refused command must not commit a document");
        assert_eq!(st.doc, before);
        assert_eq!(st.selection, selection);
        assert_eq!(history(&st.history.undo), undo);
        assert_eq!(history(&st.history.redo), redo);
        assert_eq!(st.history.costs, costs);
        assert_eq!((st.history.limit, st.history.byte_limit), limits);
        assert_eq!(st.revision, revision);
    }

    fn selected_boundary(rows: usize, cols: usize) -> (Session, ShapeId) {
        let (mut s, id) = loaded(populated(rows, cols));
        // Keep real undo and redo entries so a refusal must preserve both stacks.
        s.execute("table.options", &json!({"id": id.0, "headerRow": false})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        s.execute("table.selectCells", &json!({"id": id.0, "from": [rows - 1, 0], "to": [rows - 1, cols - 1]})).unwrap();
        assert!(!s.doc().unwrap().history.undo.is_empty() && !s.doc().unwrap().history.redo.is_empty());
        (s, id)
    }

    #[test]
    fn populated_boundary_row_growth_refuses_without_changing_state() {
        let (mut s, id) = selected_boundary(100, 1000);
        for cmd in ["table.insertRowAbove", "table.insertRowBelow"] {
            refuse_without_change(&mut s, cmd, json!({"id": id.0, "row": 0}));
            assert_eq!(table(&s, id).cell(99, 999).unwrap().text.text(), "LAST-99");
        }
    }

    #[test]
    fn populated_boundary_column_growth_refuses_without_changing_state() {
        let (mut s, id) = selected_boundary(1000, 100);
        for cmd in ["table.insertColumnLeft", "table.insertColumnRight"] {
            refuse_without_change(&mut s, cmd, json!({"id": id.0, "col": 0}));
            assert_eq!(table(&s, id).cell(999, 99).unwrap().text.text(), "LAST-999");
        }
    }

    #[test]
    fn populated_below_limit_growth_keeps_the_original_tail() {
        let (mut s, id) = loaded(populated(99, 1000));
        s.execute("table.insertRowAbove", &json!({"id": id.0, "row": 0})).unwrap();
        assert_eq!((table(&s, id).n_rows(), table(&s, id).n_cols()), (100, 1000));
        assert_eq!(table(&s, id).cell(99, 999).unwrap().text.text(), "LAST-98");
        let (mut s, id) = loaded(populated(990, 100));
        s.execute("table.insertColumnLeft", &json!({"id": id.0, "col": 0})).unwrap();
        assert_eq!((table(&s, id).n_rows(), table(&s, id).n_cols()), (990, 101));
        assert_eq!(table(&s, id).cell(989, 100).unwrap().text.text(), "LAST-989");
    }

    #[test]
    fn oversized_populated_table_edit_refuses_without_erasing_it() {
        let (mut s, id) = loaded(populated(101, 1000));
        refuse_without_change(&mut s, "table.style", json!({"id": id.0, "style": "grid"}));
        assert_eq!((table(&s, id).n_rows(), table(&s, id).n_cols()), (101, 1000));
        assert_eq!(table(&s, id).cell(100, 999).unwrap().text.text(), "LAST-100");
    }

    fn roundtrip(format: &str, t: Table) {
        let dims = (t.n_rows(), t.n_cols());
        let last = t.rows.last().unwrap().cells.last().unwrap().text.text();
        let (mut s, id) = loaded(t);
        let original = s.doc().unwrap().doc.clone();
        for _ in 0..2 {
            let before = s.doc().unwrap().doc.clone();
            let data = s.execute("file.saveBytes", &json!({"format": format})).unwrap()["data"].as_str().unwrap().to_string();
            assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().doc, &before), "saving must preserve the opened document");
            let mut opened = Session::new();
            opened.execute("file.openBytes", &json!({"name": format!("retained.{format}"), "data": data})).unwrap();
            let retained = first_table(&opened);
            assert_eq!((retained.n_rows(), retained.n_cols()), dims);
            assert_eq!(retained.rows.last().unwrap().cells.last().unwrap().text.text(), last);
            s = opened;
        }
        let ShapeKind::Table(original) = &original.slides[0].shapes.iter().find(|sh| sh.id == id).unwrap().kind else { panic!("not a table") };
        assert_eq!((original.n_rows(), original.n_cols()), dims);
        assert_eq!(original.rows.last().unwrap().cells.last().unwrap().text.text(), last);
    }

    #[test]
    fn oversized_populated_table_survives_actual_pptx_open_save_twice() {
        roundtrip("pptx", populated(101, 1000));
    }

    #[test]
    fn oversized_populated_table_survives_actual_native_open_save_twice() {
        roundtrip("deckcraft", populated(101, 1000));
    }

    #[test]
    fn oversized_ragged_table_survives_actual_pptx_open_save_twice() {
        let mut t = Table { cols: vec![10.0; 1000], rows: vec![Row { height: 30.0, cells: vec![Cell::new("stored")] }; 101], ..Default::default() };
        t.rows.last_mut().unwrap().cells[0] = Cell::new("TAIL-RAGGED");
        let (mut s, _) = loaded(t.clone());
        for _ in 0..2 {
            let data = s.execute("file.saveBytes", &json!({"format": "pptx"})).unwrap()["data"].as_str().unwrap().to_string();
            let mut opened = Session::new();
            opened.execute("file.openBytes", &json!({"name": "ragged.pptx", "data": data})).unwrap();
            let retained = first_table(&opened);
            assert_eq!((retained.n_rows(), retained.n_cols()), (101, 1000));
            assert!(retained.rows.iter().all(|r| r.cells.len() == 1));
            assert_eq!(retained.rows[100].cells[0].text.text(), "TAIL-RAGGED");
            s = opened;
        }
    }

    fn composing_table(rows: usize, cols: usize) -> (Session, ShapeId) {
        composing_loaded(populated(rows, cols))
    }

    fn composing_loaded(t: Table) -> (Session, ShapeId) {
        let (mut s, id) = loaded(t);
        s.execute("text.edit", &json!({"id": id.0, "cell": [0, 0]})).unwrap();
        s.execute("text.insert", &json!({"text": "seed"})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(!s.doc().unwrap().history.undo.is_empty() && !s.doc().unwrap().history.redo.is_empty());
        s.compose_text("x", None).unwrap();
        assert!(s.doc().unwrap().composition.is_some());
        assert!(!s.doc().unwrap().history.redo.is_empty(), "composing has not consumed redo");
        (s, id)
    }

    fn ime_refusal_preserves_state(s: &mut Session, cmd: &str, params: Value) {
        let st = s.doc().unwrap();
        let doc = st.doc.clone();
        let selection = st.selection.clone();
        let revision = st.revision;
        let composition = st.composition.clone().unwrap();
        let undo = history(&st.history.undo);
        let redo = history(&st.history.redo);
        let costs = st.history.costs.clone();
        let limits = (st.history.limit, st.history.byte_limit);
        let settles = s.composition_settles();
        let error = s.execute(cmd, &params).unwrap_err().to_string();
        assert!(error.contains(cmd) && error.contains("cell limit"), "named table-limit refusal required: {error}");
        let st = s.doc().unwrap();
        assert!(st.composition.is_some(), "a rejected table-limit operation must retain the active composition");
        let current = st.composition.as_ref().unwrap();
        assert_eq!((current.start, current.len, current.active), (composition.start, composition.len, composition.active));
        assert_eq!(current.props, composition.props);
        assert!(std::sync::Arc::ptr_eq(&current.before, &composition.before));
        assert_eq!(current.before_selection, composition.before_selection);
        assert!(std::sync::Arc::ptr_eq(&st.doc, &doc));
        assert_eq!(st.selection, selection);
        assert_eq!(st.revision, revision);
        assert_eq!(st.history.undo.len(), undo.len());
        assert_eq!(st.history.redo.len(), redo.len());
        assert_eq!(history(&st.history.undo), undo);
        assert_eq!(history(&st.history.redo), redo);
        assert_eq!(st.history.costs, costs);
        assert_eq!((st.history.limit, st.history.byte_limit), limits);
        assert_eq!(s.composition_settles(), settles, "the host must not receive a composition interrupt");
    }

    #[test]
    fn ime_limit_row_refusal_preserves_composition_and_both_history_stacks() {
        let (mut s, id) = composing_table(100, 1000);
        for cmd in ["table.insertRowAbove", "table.insertRowBelow"] {
            ime_refusal_preserves_state(&mut s, cmd, json!({"id": id.0, "row": 0}));
        }
    }

    #[test]
    fn ime_limit_column_refusal_preserves_composition_and_both_history_stacks() {
        let (mut s, _) = composing_table(1000, 100);
        for cmd in ["table.insertColumnLeft", "table.insertColumnRight"] {
            // Resolve the edited table implicitly, exactly as the command's handler does.
            ime_refusal_preserves_state(&mut s, cmd, json!({"col": 0}));
        }
    }

    #[test]
    fn ime_limit_oversized_refusal_preserves_composition_and_both_history_stacks() {
        let (mut s, id) = composing_table(101, 1000);
        ime_refusal_preserves_state(&mut s, "table.style", json!({"id": id.0, "style": "grid"}));
        ime_refusal_preserves_state(&mut s, "table.cellFill", json!({"id": id.0, "color": "#FF0000"}));
    }

    #[test]
    fn ime_limit_draw_growth_refusal_preserves_composition_and_both_history_stacks() {
        let (mut s, id) = composing_table(100, 1000);
        let x = s.doc().unwrap().shape(id).unwrap().xfrm.unwrap();
        ime_refusal_preserves_state(&mut s, "table.drawLine", json!({"id": id.0, "from": [x.x, x.y + 15.0], "to": [x.x + 400.0, x.y + 15.0]}));
    }

    #[test]
    fn ime_limit_successful_table_growth_keeps_standard_composition_undo_order() {
        let (mut s, id) = composing_table(99, 1000);
        let before_undo = s.doc().unwrap().history.undo.len();
        let settles = s.composition_settles();
        let composed = table(&s, id).cell(0, 0).unwrap().text.text();
        s.execute("table.insertRowAbove", &json!({"id": id.0, "row": 0})).unwrap();
        assert!(s.doc().unwrap().composition.is_none());
        assert_eq!(s.composition_settles(), settles + 1);
        assert_eq!(s.doc().unwrap().history.undo.len(), before_undo + 2);
        assert!(s.doc().unwrap().history.redo.is_empty());
        assert_eq!(s.doc().unwrap().history.undo[before_undo].label, "Type Text");
        assert_eq!(table(&s, id).n_rows(), 100);
        assert_eq!(table(&s, id).cell(1, 0).unwrap().text.text(), composed);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(&s, id).n_rows(), 99);
        assert_eq!(table(&s, id).cell(0, 0).unwrap().text.text(), composed);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(&s, id).cell(0, 0).unwrap().text.text(), "FIRST-0");
        s.execute("edit.redo", &json!({})).unwrap();
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(table(&s, id).n_rows(), 100);
        assert_eq!(table(&s, id).cell(1, 0).unwrap().text.text(), composed);
    }

    fn budget_refusal_preserves_ime(s: &mut Session, cmd: &str, params: Value, reason: &str) {
        let st = s.doc().unwrap();
        let doc = st.doc.clone();
        let selection = st.selection.clone();
        let revision = st.revision;
        let composition = st.composition.clone().unwrap();
        let undo = history(&st.history.undo);
        let redo = history(&st.history.redo);
        let costs = st.history.costs.clone();
        let limits = (st.history.limit, st.history.byte_limit);
        let settles = s.composition_settles();
        let error = s.execute(cmd, &params).unwrap_err().to_string();
        assert!(error.contains(cmd) && error.contains(reason), "expected budget refusal: {error}");
        let st = s.doc().unwrap();
        assert!(st.composition.is_some(), "a finite table-budget refusal must retain composition");
        let current = st.composition.as_ref().unwrap();
        assert_eq!((current.start, current.len, current.active), (composition.start, composition.len, composition.active));
        assert_eq!(current.props, composition.props);
        assert!(std::sync::Arc::ptr_eq(&current.before, &composition.before));
        assert_eq!(current.before_selection, composition.before_selection);
        assert!(std::sync::Arc::ptr_eq(&st.doc, &doc));
        assert_eq!(st.selection, selection);
        assert_eq!(st.revision, revision);
        assert_eq!(st.history.undo.len(), undo.len());
        assert_eq!(st.history.redo.len(), redo.len());
        assert_eq!(history(&st.history.undo), undo);
        assert_eq!(history(&st.history.redo), redo);
        assert_eq!(st.history.costs, costs);
        assert_eq!((st.history.limit, st.history.byte_limit), limits);
        assert_eq!(s.composition_settles(), settles);
    }

    fn budget_success_keeps_ime_order(s: &mut Session, id: ShapeId, cmd: &str, params: Value) {
        let before_undo = s.doc().unwrap().history.undo.len();
        let settles = s.composition_settles();
        let composed = table(s, id).cell(0, 0).unwrap().text.text();
        let original = s.doc().unwrap().composition.as_ref().unwrap().before.slides[0].shapes.iter().find(|sh| sh.id == id).unwrap();
        let ShapeKind::Table(original) = &original.kind else { panic!("not a table") };
        let original = original.cell(0, 0).unwrap().text.text();
        s.execute(cmd, &params).unwrap();
        assert!(s.doc().unwrap().composition.is_none());
        assert_eq!(s.composition_settles(), settles + 1);
        assert_eq!(s.doc().unwrap().history.undo.len(), before_undo + 2);
        assert_eq!(s.doc().unwrap().history.undo[before_undo].label, "Type Text");
        assert!(s.doc().unwrap().history.redo.is_empty());
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(s, id).cell(0, 0).unwrap().text.text(), composed);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(s, id).cell(0, 0).unwrap().text.text(), original);
        s.execute("edit.redo", &json!({})).unwrap();
        s.execute("edit.redo", &json!({})).unwrap();
    }

    #[test]
    fn budget_ime_merge_work_refusal_preserves_complete_state() {
        let mut t = populated(1000, 10);
        for row in &mut t.rows {
            for cell in &mut row.cells {
                cell.row_span = 2;
            }
        }
        let (mut s, id) = composing_loaded(t);
        budget_refusal_preserves_ime(&mut s, "table.merge", json!({"id": id.0, "from": [998, 0], "to": [999, 9]}), "can't merge");
    }

    #[test]
    fn budget_ime_explicit_list_refusal_covers_every_target_style_command() {
        let (mut s, id) = composing_table(1, 1);
        let cells = vec![json!([0, 0]); MAX_CELLS + 1];
        for (cmd, mut p) in [
            ("table.cellFill", json!({"color": "#FF0000"})),
            ("table.borders", json!({"which": "all"})),
            ("table.cellMargins", json!({"left": 4})),
            ("table.cellAlign", json!({"align": "center"})),
        ] {
            p["id"] = json!(id.0);
            p["cells"] = json!(cells);
            budget_refusal_preserves_ime(&mut s, cmd, p, "at most 100000");
        }
    }

    #[test]
    fn budget_ime_draw_grid_cap_refusal_preserves_complete_state() {
        let (mut s, id) = composing_table(200, 2);
        let x = s.doc().unwrap().shape(id).unwrap().xfrm.unwrap();
        budget_refusal_preserves_ime(
            &mut s,
            "table.drawLine",
            json!({"id": id.0, "from": [x.x, x.y + 15.0], "to": [x.x + 400.0, x.y + 15.0]}),
            "doesn't divide",
        );
    }

    #[test]
    fn budget_ime_merge_at_exact_work_limit_keeps_normal_success_order() {
        let mut t = populated(400, 10);
        for row in &mut t.rows {
            for cell in &mut row.cells {
                cell.row_span = 2;
            }
        }
        let (mut s, id) = composing_loaded(t);
        // 400 scans x 4000 cells = exactly the unchanged 1600000-visit budget.
        budget_success_keeps_ime_order(&mut s, id, "table.merge", json!({"id": id.0, "from": [399, 0], "to": [399, 9]}));
    }

    #[test]
    fn budget_ime_target_list_at_cap_keeps_normal_success_for_all_styles() {
        let cells = vec![json!([0, 0]); MAX_CELLS];
        for (cmd, mut p) in [
            ("table.cellFill", json!({"color": "#FF0000"})),
            ("table.borders", json!({"which": "all"})),
            ("table.cellMargins", json!({"left": 4})),
            ("table.cellAlign", json!({"align": "center"})),
        ] {
            let (mut s, id) = composing_table(1, 1);
            p["id"] = json!(id.0);
            p["cells"] = json!(cells);
            budget_success_keeps_ime_order(&mut s, id, cmd, p);
        }
    }

    #[test]
    fn budget_ime_draw_grid_boundary_success_keeps_normal_order() {
        let (mut s, id) = composing_table(199, 2);
        let x = s.doc().unwrap().shape(id).unwrap().xfrm.unwrap();
        budget_success_keeps_ime_order(&mut s, id, "table.drawLine", json!({"id": id.0, "from": [x.x, x.y + 15.0], "to": [x.x + 400.0, x.y + 15.0]}));
        assert_eq!((table(&s, id).n_rows(), table(&s, id).n_cols()), (200, 2));
    }

    #[test]
    fn budget_ime_default_and_selected_range_style_targets_keep_success_order() {
        for range in [false, true] {
            let (mut s, id) = composing_table(2, 2);
            if range {
                s.select(|_, sel| sel.cells = Some((id, (0, 0), (1, 1)))).unwrap();
            }
            budget_success_keeps_ime_order(&mut s, id, "table.cellMargins", json!({"id": id.0, "left": 4}));
        }
    }

    #[test]
    fn budget_structural_preview_matches_real_geometry_without_cell_payloads() {
        let geometry = |t: &Table| {
            (
                t.cols.clone(),
                t.rows
                    .iter()
                    .map(|r| (r.height, r.cells.iter().map(|c| (c.grid_span, c.row_span, c.h_merge, c.v_merge)).collect::<Vec<_>>()))
                    .collect::<Vec<_>>(),
            )
        };
        for (rows, cols) in [(2, 2), (199, 2), (200, 2), (100, 1000)] {
            let mut t = populated(rows, cols);
            t.cell_mut(0, 0).unwrap().text = deckcraft_model::TextBody::from_text(&"payload".repeat(10000));
            for (from, to) in [((0.0, 15.0), (400.0, 15.0)), ((100.0, 0.0), (100.0, 60.0)), ((0.0, 1.0), (400.0, 1.0))] {
                let mut structural = budget_grid(&t);
                assert_eq!(geometry(&structural), geometry(&t));
                assert!(structural.rows.iter().flat_map(|r| &r.cells).all(|c| c.text.is_empty() && c.fill.is_none()));
                let mut actual = t.clone();
                actual.normalize();
                structural.normalize();
                let actual_changed = crate::drawtable::draw_line(&mut actual, from, to);
                let planned_changed = crate::drawtable::draw_line(&mut structural, from, to);
                assert_eq!(actual_changed, planned_changed);
                assert_eq!(geometry(&actual), geometry(&structural));
                assert_eq!(check_draw_admission(&t, from, to, "table.drawLine").is_ok(), actual_changed && actual.can_normalize());
            }
        }
    }

    fn format_work_chain(rows: usize) -> (Session, ShapeId) {
        let mut t = Table::new(rows, 10, 400.0, 30.0);
        for (r, row) in t.rows.iter_mut().enumerate() {
            for (c, cell) in row.cells.iter_mut().enumerate() {
                cell.row_span = 2;
                cell.text = deckcraft_model::TextBody::from_text(&format!("CELL-{r}-{c}"));
            }
        }
        assert!(t.is_normalized() && t.can_normalize());
        let (mut s, id) = composing_loaded(t);
        s.select(|_, sel| sel.cells = Some((id, (rows - 1, 0), (rows - 1, 9)))).unwrap();
        assert!(s.doc().unwrap().composition.is_some());
        assert!(!s.doc().unwrap().history.undo.is_empty() && !s.doc().unwrap().history.redo.is_empty());
        (s, id)
    }

    fn format_work_params(cmd: &str, id: ShapeId) -> Value {
        let mut p = match cmd {
            "table.cellFill" => json!({"color": "#112233"}),
            "table.borders" => json!({"which": "all", "color": "#112233", "width": 2}),
            "table.cellMargins" => json!({"left": 4, "right": 5, "top": 6, "bottom": 7}),
            "table.cellAlign" => json!({"align": "center", "anchor": "bottom"}),
            _ => panic!("not a formatting test command"),
        };
        p["id"] = json!(id.0);
        p
    }

    fn format_work_all_formatted(t: &Table, cmd: &str) {
        let color = deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::rgb(0x11, 0x22, 0x33));
        let line = deckcraft_model::Line::solid(color.clone(), 2.0);
        assert_eq!((t.n_rows(), t.n_cols()), (400, 10));
        for (r, row) in t.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                match cmd {
                    "table.cellFill" => assert_eq!(cell.fill, Some(deckcraft_model::Fill::solid(color.clone())), "missing fill at {r},{c}"),
                    "table.borders" => assert!(cell.borders.iter().all(|b| b.as_ref() == Some(&line)), "missing border at {r},{c}"),
                    "table.cellMargins" => assert_eq!(cell.margins, Some([4.0, 5.0, 6.0, 7.0]), "missing margins at {r},{c}"),
                    "table.cellAlign" => {
                        assert_eq!(cell.anchor, Some(deckcraft_model::text::Anchor::Bottom), "missing anchor at {r},{c}");
                        assert!(!cell.text.paragraphs.is_empty());
                        assert!(
                            cell.text.paragraphs.iter().all(|p| p.props.align == Some(deckcraft_model::text::Align::Center)),
                            "missing align at {r},{c}"
                        );
                    }
                    _ => panic!("not a formatting test command"),
                }
            }
        }
    }

    fn format_work_refusal(cmd: &str) {
        let (mut s, id) = format_work_chain(401);
        budget_refusal_preserves_ime(&mut s, cmd, format_work_params(cmd, id), "selected cell expansion work limit");
    }

    fn format_work_boundary(cmd: &str) {
        let (mut s, id) = format_work_chain(400);
        let before = table(&s, id).clone();
        let original = s.doc().unwrap().composition.as_ref().unwrap().before.slides[0].shapes.iter().find(|sh| sh.id == id).unwrap();
        let ShapeKind::Table(original) = &original.kind else { panic!("not a table") };
        let original = original.clone();
        let undo = history(&s.doc().unwrap().history.undo);
        let settles = s.composition_settles();
        let label = s.commands().into_iter().find(|c| c.id == cmd).unwrap().label.to_string();
        s.execute(cmd, &format_work_params(cmd, id)).unwrap();
        assert!(s.doc().unwrap().composition.is_none());
        assert_eq!(s.composition_settles(), settles + 1);
        let entries = history(&s.doc().unwrap().history.undo);
        assert_eq!(entries.len(), undo.len() + 2);
        assert_eq!(&entries[..undo.len()], undo.as_slice());
        assert_eq!(entries[undo.len()].0, "Type Text");
        assert_eq!(entries[undo.len() + 1].0, label);
        assert!(s.doc().unwrap().history.redo.is_empty());
        format_work_all_formatted(table(&s, id), cmd);
        let formatted = table(&s, id).clone();
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(&s, id), &before, "first undo removes the complete formatting command");
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(table(&s, id), &original, "second undo removes only the composition");
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(table(&s, id), &before);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(table(&s, id), &formatted);
        assert_eq!(s.composition_settles(), settles + 1);
    }

    #[test]
    fn format_work_refusal_fill_preserves_complete_state() {
        format_work_refusal("table.cellFill");
    }
    #[test]
    fn format_work_refusal_borders_preserves_complete_state() {
        format_work_refusal("table.borders");
    }
    #[test]
    fn format_work_refusal_margins_preserves_complete_state() {
        format_work_refusal("table.cellMargins");
    }
    #[test]
    fn format_work_refusal_align_preserves_complete_state() {
        format_work_refusal("table.cellAlign");
    }
    #[test]
    fn format_work_boundary_fill_formats_every_cell_in_normal_ime_order() {
        format_work_boundary("table.cellFill");
    }
    #[test]
    fn format_work_boundary_borders_format_every_cell_in_normal_ime_order() {
        format_work_boundary("table.borders");
    }
    #[test]
    fn format_work_boundary_margins_format_every_cell_in_normal_ime_order() {
        format_work_boundary("table.cellMargins");
    }
    #[test]
    fn format_work_boundary_align_formats_every_cell_in_normal_ime_order() {
        format_work_boundary("table.cellAlign");
    }

    #[test]
    fn format_work_range_semantics_match_legacy_for_reversed_clamped_single_and_sparse_tables() {
        let mut dense = Table::new(4, 3, 120.0, 30.0);
        dense.cell_mut(1, 1).unwrap().row_span = 2;
        dense.cell_mut(1, 1).unwrap().grid_span = 2;
        dense.cell_mut(0, 0).unwrap().grid_span = 0;
        dense.cell_mut(0, 0).unwrap().row_span = 0;
        dense.cell_mut(3, 2).unwrap().h_merge = true;
        let mut sparse = dense.clone();
        sparse.rows[0].cells.truncate(1);
        sparse.rows[1].cells.truncate(2);
        sparse.rows[2].cells.clear();
        for raw in [dense, sparse] {
            let mut normalized = raw.clone();
            assert!(normalized.try_normalize());
            for (a, b) in
                [((3, 2), (0, 0)), ((usize::MAX, usize::MAX), (0, 0)), ((2, 1), (2, 1)), ((3, 0), (0, 2)), ((0, 0), (0, 0)), ((99, 99), (99, 99))]
            {
                let expected = cell_block(&normalized, a, b);
                assert_eq!(formatting_cell_block(&raw, a, b, "table.cellFill").unwrap(), expected);
                let (mut s, id) = loaded(raw.clone());
                s.select(|_, sel| sel.cells = Some((id, a, b))).unwrap();
                s.execute("table.cellFill", &json!({"id": id.0, "color": "#112233"})).unwrap();
                let after = table(&s, id);
                assert!(after.is_normalized());
                let (lo, hi) = (
                    (expected.0.0.min(expected.1.0), expected.0.1.min(expected.1.1)),
                    (expected.0.0.max(expected.1.0), expected.0.1.max(expected.1.1)),
                );
                for (r, row) in after.rows.iter().enumerate() {
                    for (c, cell) in row.cells.iter().enumerate() {
                        assert_eq!(cell.fill.is_some(), (lo.0..=hi.0).contains(&r) && (lo.1..=hi.1).contains(&c), "wrong target at {r},{c}");
                        assert_eq!(cell.text, normalized.rows[r].cells[c].text);
                    }
                }
            }
        }
    }

    #[test]
    fn format_work_preserves_explicit_list_caret_default_and_actual_id_precedence() {
        for cmd in ["table.cellFill", "table.borders", "table.cellMargins", "table.cellAlign"] {
            let (mut s, id) = format_work_chain(401);
            let mut params = format_work_params(cmd, id);
            params["cells"] = json!([[0, 0]]);
            budget_success_keeps_ime_order(&mut s, id, cmd, params);
            let tail = table(&s, id).cell(400, 9).unwrap();
            assert!(tail.fill.is_none() && tail.borders.iter().all(Option::is_none) && tail.margins.is_none() && tail.anchor.is_none());
        }
        let (mut s, id) = format_work_chain(401);
        s.select(|_, sel| sel.cells = None).unwrap();
        budget_success_keeps_ime_order(&mut s, id, "table.cellFill", format_work_params("table.cellFill", id));
        assert!(table(&s, id).cell(0, 0).unwrap().fill.is_some());
        assert!(table(&s, id).cell(400, 9).unwrap().fill.is_none());
        let (mut s, id) = format_work_chain(401);
        s.end_composition();
        s.select(|_, sel| {
            sel.cells = None;
            sel.text = None;
        })
        .unwrap();
        s.execute("table.cellFill", &format_work_params("table.cellFill", id)).unwrap();
        assert!(table(&s, id).rows.iter().flat_map(|r| &r.cells).all(|c| c.fill.is_some()));
        let (mut s, big) = format_work_chain(401);
        s.end_composition();
        let small = ShapeId(s.execute("insert.table", &json!({"rows": 2, "cols": 2})).unwrap()["id"].as_u64().unwrap() as u32);
        s.select(|_, sel| {
            sel.shapes = vec![big];
            sel.text = None;
            sel.cells = Some((big, (400, 0), (400, 9)));
        })
        .unwrap();
        s.execute("table.cellFill", &format_work_params("table.cellFill", small)).unwrap();
        assert!(table(&s, big).rows.iter().flat_map(|r| &r.cells).all(|c| c.fill.is_none()));
        assert!(table(&s, small).rows[0].cells.iter().all(|c| c.fill.is_none()));
        assert!(table(&s, small).rows[1].cells.iter().all(|c| c.fill.is_some()));
    }
}
