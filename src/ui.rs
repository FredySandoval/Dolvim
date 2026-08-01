//! Rendering. The layout here is the screenshot; `docs/UI_SPEC.md` records
//! which pixel produced which constant.

use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, MenuKind, Mode, Pane, ViewMode};
use crate::config::{self, color, glyph as g};
use crate::fs::{self, SortKey};
use crate::places::Row;
use crate::vim;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Paint the whole window in the view background first, so the gaps
    // between widgets are Breeze white, not terminal default.
    f.render_widget(
        Block::default().style(Style::default().bg(color::VIEW_BG)),
        area,
    );

    let tabbar = if app.tabs.len() > 1 { 1 } else { 0 };
    let filter = if app.filter_bar { 1 } else { 0 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),      // toolbar + breadcrumb
            Constraint::Length(tabbar), // tab bar, only with >1 tab
            Constraint::Min(1),         // body
            Constraint::Length(filter), // filter bar
            Constraint::Length(1),      // status bar
        ])
        .split(area);

    toolbar(f, app, rows[0]);
    if tabbar == 1 {
        tab_bar(f, app, rows[1]);
    }
    body(f, app, rows[2]);
    if filter == 1 {
        filter_bar(f, app, rows[3]);
    }
    status_bar(f, app, rows[4]);

    overlays(f, app, area);
}

// ---------------------------------------------------------------------------
// Toolbar and breadcrumb
// ---------------------------------------------------------------------------

/// Every ancestor of `p`, root first, as whole paths.
pub fn crumb_paths(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = p.ancestors().map(Path::to_path_buf).collect();
    v.reverse();
    v
}

/// Dolphin shows `Home` rather than the literal home path, and hides the
/// leading components of anything below it.
fn crumb_label(p: &Path) -> String {
    let home = crate::places::home();
    if p == home {
        return "Home".into();
    }
    if p == Path::new("/") {
        return "/".into();
    }
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn toolbar(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(color::TOOLBAR_BG)),
        area,
    );
    // Where the text cursor should end up, applied once the buffer borrow ends.
    let mut cursor: Option<(u16, u16)> = None;
    let buf = f.buffer_mut();
    let base = Style::default().bg(color::TOOLBAR_BG).fg(color::TEXT);
    let dim = base.fg(color::DIM);
    let p = app.pane();
    let can_back = p.hist_pos > 0;
    let can_fwd = p.hist_pos + 1 < p.history.len();

    // The focused toolbar button wears the same fill a selected file does.
    let sel = |n: usize, st: Style| match app.mode {
        Mode::Buttons(i) if i == n => st.bg(color::SELECTION),
        _ => st,
    };

    let mut x = area.x;
    let put = |buf: &mut Buffer, s: &str, st: Style, x: &mut u16| -> Rect {
        let w = s.width() as u16;
        buf.set_string(*x, area.y, s, st);
        let r = Rect::new(*x, area.y, w, 1);
        *x += w;
        r
    };

    put(buf, " ", base, &mut x);
    let back_st = sel(0, if can_back { base } else { dim });
    app.hits.back = put(buf, g::BACK, back_st, &mut x);
    put(buf, "  ", base, &mut x);
    let fwd_st = sel(1, if can_fwd { base } else { dim });
    app.hits.forward = put(buf, g::FORWARD, fwd_st, &mut x);
    put(buf, "   ", base, &mut x);

    // Dolphin's split button: the icon shows the current mode and cycles it,
    // the caret beside it opens the full list. Two hitboxes, not one.
    let vg = match app.pane().view {
        ViewMode::Icons => g::VIEW_ICONS,
        ViewMode::Compact => g::VIEW_COMPACT,
        ViewMode::Details => g::VIEW_DETAILS,
    };
    let view_st = sel(2, base);
    app.hits.view_cycle = put(buf, vg, view_st, &mut x);
    put(buf, " ", view_st, &mut x);
    app.hits.view_menu = put(buf, g::DROPDOWN, view_st, &mut x);

    // The navigation group sits over the Places panel and the breadcrumb
    // starts where the file view does, which is how Dolphin lines them up.
    if app.places_visible {
        x = x.max(area.x + config::PLACES_WIDTH);
    }

    // Breadcrumb, or the editable path field when path edit is active.
    let right_w: u16 = 22;
    let crumb_area = Rect::new(
        x,
        area.y,
        area.width
            .saturating_sub(x - area.x)
            .saturating_sub(right_w),
        1,
    );
    app.hits.path_bar = crumb_area;
    app.hits.crumbs.clear();
    app.hits.crumb_arrows.clear();

    if app.mode == Mode::PathEdit {
        let text = format!(" {}", app.input);
        buf.set_string(
            crumb_area.x,
            area.y,
            clip(&text, crumb_area.width as usize),
            base.bg(color::VIEW_BG),
        );
        cursor = Some((crumb_area.x + 1 + app.input_cursor as u16, area.y));
    } else {
        let paths = crumb_paths(&app.pane().cwd);
        // Dolphin elides from the left when the trail does not fit.
        let mut segs: Vec<(usize, String)> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (i, crumb_label(p)))
            .collect();
        while total_crumb_width(&segs) > crumb_area.width && segs.len() > 1 {
            segs.remove(0);
        }
        // The segment whose dropdown is open, if any. Its arrow points down and
        // it carries the selection background, so the row says where you are.
        let open_seg = match app.mode {
            Mode::CrumbMenu(i) => Some(i),
            _ => None,
        };
        let mut cx = crumb_area.x;
        let last = segs.len().saturating_sub(1);
        for (n, (i, label)) in segs.iter().enumerate() {
            // One marker at the head of the trail, then blanks. A separator per
            // hop is three columns each of punctuation the path already implies.
            let sep = if n == 0 {
                format!(" {} ", g::CRUMB_SEP)
            } else {
                "  ".to_string()
            };
            buf.set_string(cx, area.y, &sep, dim);
            cx += sep.width() as u16;
            let open = open_seg == Some(*i);
            let st = if open {
                base.bg(color::SELECTION)
            } else if n == last {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };
            let w = label.width() as u16;
            if cx + w > crumb_area.right() {
                break;
            }
            buf.set_string(cx, area.y, label, st);
            app.hits
                .crumbs
                .push((Rect::new(cx, area.y, w, 1), paths[*i].clone()));
            cx += w;
            let arrow = Rect::new(cx, area.y, 1, 1);
            let (glyph, ast) = if open {
                (g::DROPDOWN, st)
            } else {
                (g::CRUMB_SHUT, dim)
            };
            buf.set_string(cx, area.y, glyph, ast);
            app.hits.crumb_arrows.push((arrow, *i));
            cx += 1;
        }
    }

    // Right-aligned controls, laid out from the edge inward.
    let mut rx = area.right();
    let put_r = |buf: &mut Buffer, s: &str, st: Style, rx: &mut u16| -> Rect {
        let w = s.width() as u16;
        *rx = rx.saturating_sub(w);
        buf.set_string(*rx, area.y, s, st);
        Rect::new(*rx, area.y, w, 1)
    };
    put_r(buf, " ", base, &mut rx);
    app.hits.menu = put_r(buf, g::MENU, sel(5, base), &mut rx);
    put_r(buf, "   ", base, &mut rx);
    app.hits.search = put_r(buf, g::SEARCH, sel(4, base), &mut rx);
    put_r(buf, "   ", base, &mut rx);
    let split_style = sel(
        3,
        if app.split_on() {
            base.fg(color::ACCENT)
        } else {
            base
        },
    );
    app.hits.split = put_r(buf, &format!("{} Split", g::SPLIT), split_style, &mut rx);

    if let Some(pos) = cursor {
        f.set_cursor_position(pos);
    }
}

/// Label plus arrow plus the blanks in front: three columns for the leading
/// ` › `, two for every hop after it.
fn total_crumb_width(segs: &[(usize, String)]) -> u16 {
    segs.iter()
        .enumerate()
        .map(|(n, (_, s))| s.width() as u16 + 1 + if n == 0 { 3 } else { 2 })
        .sum()
}

fn tab_bar(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(color::PANEL_BG)),
        area,
    );
    let buf = f.buffer_mut();
    app.hits.tabs.clear();
    let mut x = area.x;
    for (i, t) in app.tabs.iter().enumerate() {
        let label = format!(" {} ", t.title());
        let w = label.width() as u16;
        if x + w > area.right() {
            break;
        }
        let st = if i == app.tab {
            Style::default()
                .bg(color::VIEW_BG)
                .fg(color::TEXT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(color::PANEL_BG).fg(color::DIM)
        };
        buf.set_string(x, area.y, &label, st);
        app.hits.tabs.push(Rect::new(x, area.y, w, 1));
        x += w;
    }
}

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

fn body(f: &mut Frame, app: &mut App, area: Rect) {
    let pw = if app.places_visible {
        config::PLACES_WIDTH
    } else {
        0
    };
    let iw = if app.info_visible {
        config::INFO_WIDTH
    } else {
        0
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(pw),
            Constraint::Min(10),
            Constraint::Length(iw),
        ])
        .split(area);

    if pw > 0 {
        places_panel(f, app, cols[0]);
    }
    app.hits.places = cols[0];

    let n = app.tab().panes.len();
    if n > 1 {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(cols[1]);
        for i in 0..n {
            view(f, app, halves[i], i);
        }
        // The divider between the two views.
        let x = halves[1].x.saturating_sub(1);
        for y in cols[1].y..cols[1].bottom() {
            if let Some(c) = f.buffer_mut().cell_mut((x, y)) {
                c.set_char('\u{2502}').set_fg(color::SEPARATOR);
            }
        }
    } else {
        view(f, app, cols[1], 0);
    }

    if iw > 0 {
        info_panel(f, app, cols[2]);
    }
}

fn places_panel(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(color::PANEL_BG)),
        area,
    );
    let focused = app.focus == Focus::Places;
    let buf = f.buffer_mut();
    // No splitter rule: PANEL_BG against the view's white already reads as an
    // edge, and a drawn line only competes with it. Full width is ours.
    let w = area.width;

    for (i, row) in app.places.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.bottom() {
            break;
        }
        match row {
            Row::Gap => {}
            Row::Heading(h) => {
                buf.set_string(
                    area.x + 1,
                    y,
                    clip(h, w.saturating_sub(1) as usize),
                    Style::default().bg(color::PANEL_BG).fg(color::DIM),
                );
            }
            Row::Item {
                label,
                glyph,
                gauge,
                offline,
                eject,
                ..
            } => {
                let selected = i == app.places_sel;
                let bg = if selected {
                    color::SELECTION
                } else {
                    color::PANEL_BG
                };
                let st = Style::default().bg(bg).fg(color::TEXT);
                for x in area.x..area.x + w {
                    if let Some(c) = buf.cell_mut((x, y)) {
                        c.set_char(' ').set_style(st);
                    }
                }
                // Devices carry a free-space gauge behind the label.
                if let Some((used, total)) = gauge {
                    if *total > 0 {
                        let filled = ((*used as f64 / *total as f64) * w as f64) as u16;
                        for x in area.x..area.x + filled.min(w) {
                            if let Some(c) = buf.cell_mut((x, y)) {
                                c.set_bg(if selected {
                                    color::SELECTION
                                } else {
                                    color::GAUGE_FULL
                                });
                            }
                        }
                    }
                }
                // Foreground only: leaving `bg` unset lets the gauge painted
                // above show through instead of being punched out per glyph.
                // Dolphin badges an unreachable disk on its icon and leaves
                // the label alone, so the row still reads as ordinary text.
                let fg = if *offline {
                    color::OFFLINE
                } else {
                    color::TEXT
                };
                buf.set_string(area.x + 1, y, glyph, Style::default().fg(fg));
                let gw = glyph.width().max(1) as u16 + 1;
                // The eject affordance sits at the right edge, so the label
                // has to give up its columns before it is clipped, not after.
                let ew = if *eject {
                    g::EJECT.width().max(1) as u16 + 1
                } else {
                    0
                };
                buf.set_string(
                    area.x + 1 + gw,
                    y,
                    clip(label, w.saturating_sub(2 + gw + ew) as usize),
                    Style::default().fg(color::TEXT),
                );
                if *eject {
                    buf.set_string(
                        area.x + w.saturating_sub(ew),
                        y,
                        g::EJECT,
                        Style::default().fg(color::DIM),
                    );
                }
                if selected && focused {
                    if let Some(c) = buf.cell_mut((area.x, y)) {
                        c.set_char('\u{2590}').set_fg(color::ACCENT);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// File views
// ---------------------------------------------------------------------------

fn view(f: &mut Frame, app: &mut App, area: Rect, idx: usize) {
    let is_active = idx == app.tab().active && app.focus == Focus::View;
    f.render_widget(
        Block::default().style(Style::default().bg(color::VIEW_BG)),
        area,
    );
    {
        let p = app.pane_at_mut(idx);
        p.area = area;
    }

    let mode = app.pane_at(idx).view;
    match mode {
        ViewMode::Icons => icons_view(f, app, area, idx, is_active),
        ViewMode::Compact => compact_view(f, app, area, idx, is_active),
        ViewMode::Details => details_view(f, app, area, idx, is_active),
    }

    let p = app.pane_at(idx);
    if p.loading && p.entries.is_empty() {
        centred(
            f.buffer_mut(),
            area,
            "Loading…",
            Style::default().fg(color::DIM).bg(color::VIEW_BG),
        );
    } else if let Some(e) = &p.error {
        let msg = e.clone();
        centred(
            f.buffer_mut(),
            area,
            &msg,
            Style::default().fg(color::ERROR).bg(color::VIEW_BG),
        );
    } else if p.is_empty() {
        let msg = if p.filter.is_empty() {
            "This folder is empty."
        } else {
            "No items match the filter."
        };
        centred(
            f.buffer_mut(),
            area,
            msg,
            Style::default().fg(color::DIM).bg(color::VIEW_BG),
        );
    }
}

fn entry_style(p: &Pane, vis: usize, cut: bool) -> Style {
    let e = &p.entries[p.visible[vis]];
    let selected = p.selected.contains(&e.path);
    let fg = if cut {
        color::CUT
    } else if e.is_locked() {
        color::OFFLINE
    } else if e.is_dir() {
        color::FOLDER
    } else if e.kind == fs::Kind::Symlink {
        color::SYMLINK
    } else if e.is_executable() {
        color::EXEC
    } else {
        color::TEXT
    };
    let bg = if selected {
        color::SELECTION
    } else {
        color::VIEW_BG
    };
    Style::default().fg(fg).bg(bg)
}

/// Scroll to the cursor, but only when the cursor is what moved.
///
/// Dolphin's wheel scrolls the view and leaves the selection alone, even off
/// screen, so the renderer must not drag the viewport back on the next frame.
/// A view change is in the key, since it reorients the axis the cursor sits on.
fn reveal(p: &mut Pane, line: usize, visible: usize) {
    let state = (p.cursor, p.view);
    if p.last_reveal == state {
        return;
    }
    p.last_reveal = state;
    scroll_to(&mut p.offset, line, visible);
}

/// Scroll so `cursor` is on screen, in units of whichever axis scrolls.
fn scroll_to(offset: &mut usize, cursor_line: usize, visible_lines: usize) {
    if cursor_line < *offset {
        *offset = cursor_line;
    } else if visible_lines > 0 && cursor_line >= *offset + visible_lines {
        *offset = cursor_line + 1 - visible_lines;
    }
}

/// Columns, rows and left margin of the icon grid.
///
/// A cell carries its own margin: a blank row above it and CELL_GAP blank
/// columns. The cursor frame is drawn in that margin, so it costs no row of
/// content and no two names can touch. Its bottom edge lands on the blank row
/// the cell below starts with, which is why the grid keeps one row spare.
pub fn icon_grid(area: Rect) -> (u16, u16, u16) {
    let (cw, ch) = (config::CELL_W, config::CELL_H);
    let cols = (area.width.saturating_sub(2) / cw).max(1);
    let rows = (area.height.saturating_sub(1) / ch).max(1);
    // Columns never divide the pane evenly. The remainder becomes margin, split
    // between the two sides, so the grid stays centred as the pane is resized.
    (cols, rows, area.width.saturating_sub(cols * cw) / 2)
}

fn icons_view(f: &mut Frame, app: &mut App, area: Rect, idx: usize, active: bool) {
    let (cw, ch) = (config::CELL_W, config::CELL_H);
    let (cols, rows, mx) = icon_grid(area);
    let cut = app.clipboard.cut;
    let cut_set = app.clipboard.paths.clone();
    {
        let p = app.pane_at_mut(idx);
        p.grid_cols = cols;
        p.grid_rows = rows;
        p.cell_w = cw;
        p.cell_h = ch;
        p.grid_x = area.x + mx;
        let cur_row = p.cursor / cols as usize;
        reveal(p, cur_row, rows as usize);
    }
    let p_len = app.pane_at(idx).visible.len();
    let offset = app.pane_at(idx).offset;
    let first = offset * cols as usize;

    let gap = config::CELL_GAP.min(cw.saturating_sub(3));
    let tile_w = cw - gap;
    let body_h = ch - 1;
    // The name always gets its rows; the icon takes what is left.
    let name_lines = config::NAME_LINES.clamp(1, body_h.saturating_sub(1).max(1));
    let icon_h = body_h.saturating_sub(name_lines);

    for slot in 0..(cols as usize * rows as usize) {
        let vis = first + slot;
        if vis >= p_len {
            break;
        }
        let (r, c) = (slot / cols as usize, slot % cols as usize);
        let x0 = area.x + mx + c as u16 * cw;
        let y0 = area.y + r as u16 * ch;
        let cell = Rect::new(
            x0,
            y0,
            cw.min(area.right().saturating_sub(x0)),
            ch.min(area.bottom().saturating_sub(y0)),
        );
        if cell.width == 0 || cell.height == 0 {
            continue;
        }
        let e = {
            let p = app.pane_at(idx);
            p.entries[p.visible[vis]].clone()
        };
        let is_cut = cut && cut_set.contains(&e.path);
        let st = entry_style(app.pane_at(idx), vis, is_cut);

        // Selection fill covers the whole cell.
        if st.bg == Some(color::SELECTION) {
            fill(f.buffer_mut(), cell, color::SELECTION);
        }

        // Thumbnail, or the glyph stand-in while one is being decoded.
        let body = Rect::new(
            cell.x + gap / 2,
            cell.y + 1,
            tile_w.min(cell.width.saturating_sub(gap / 2)),
            cell.height.saturating_sub(1),
        );
        let thumb_area = Rect::new(body.x, body.y, body.width, icon_h.min(body.height));
        let drew = if e.is_image() && thumb_area.width > 1 && thumb_area.height > 0 {
            let t = app
                .thumbs
                .get(&e.path, thumb_area.width, thumb_area.height)
                .cloned();
            match t {
                Some(t) => {
                    blit(f.buffer_mut(), thumb_area, &t);
                    true
                }
                None => false,
            }
        } else {
            false
        };
        if !drew {
            // The icon's colour says what the entry is, not whether it is
            // selected, so it is the entry's kind that picks it.
            let fg = if is_cut {
                color::CUT
            } else if e.is_locked() {
                color::OFFLINE
            } else if e.is_dir() {
                color::FOLDER
            } else {
                color::FILE
            };
            centred(f.buffer_mut(), thumb_area, e.glyph(), st.fg(fg));
        }

        // Name, wrapped over its rows and centred, as Dolphin centres it.
        let name = wrap_name(&e.name, body.width as usize, name_lines as usize);
        for (li, part) in name.iter().enumerate() {
            let y = body.y + icon_h + li as u16;
            if y >= body.bottom() {
                break;
            }
            let x = body.x + body.width.saturating_sub(part.width() as u16) / 2;
            f.buffer_mut().set_string(x, y, part, st);
        }

        if vis == app.pane_at(idx).cursor {
            // The frame hugs what is actually drawn: the blank margin row it is
            // hung from, the icon, and only the name rows this name used. A
            // one-line name gets a short box instead of two rows of empty space,
            // and its bottom edge falls on a row the cell was not using anyway.
            let used = (name.len() as u16).max(1);
            let h = (icon_h + used + 2).min(area.bottom().saturating_sub(y0));
            outline(
                f.buffer_mut(),
                Rect::new(cell.x, cell.y, cell.width, h),
                active,
            );
        }
    }
}

fn compact_view(f: &mut Frame, app: &mut App, area: Rect, idx: usize, active: bool) {
    let cw = (config::CELL_W + 4).min(area.width.max(1));
    let rows = area.height.max(1);
    let cols = (area.width / cw).max(1);
    let cut = app.clipboard.cut;
    let cut_set = app.clipboard.paths.clone();
    {
        let p = app.pane_at_mut(idx);
        p.grid_cols = cols;
        p.grid_rows = rows;
        p.cell_w = cw;
        p.cell_h = 1;
        // Compact flows down columns, so the scroll axis is columns.
        let cur_col = p.cursor / rows as usize;
        reveal(p, cur_col, cols as usize);
    }
    let p_len = app.pane_at(idx).visible.len();
    let offset = app.pane_at(idx).offset;
    let first = offset * rows as usize;

    for slot in 0..(cols as usize * rows as usize) {
        let vis = first + slot;
        if vis >= p_len {
            break;
        }
        let (c, r) = (slot / rows as usize, slot % rows as usize);
        let x = area.x + c as u16 * cw;
        let y = area.y + r as u16;
        if y >= area.bottom() || x >= area.right() {
            continue;
        }
        let e = {
            let p = app.pane_at(idx);
            p.entries[p.visible[vis]].clone()
        };
        let is_cut = cut && cut_set.contains(&e.path);
        let st = entry_style(app.pane_at(idx), vis, is_cut);
        let w = cw.min(area.right() - x);
        let cell = Rect::new(x, y, w, 1);
        if st.bg == Some(color::SELECTION) {
            fill(f.buffer_mut(), cell, color::SELECTION);
        }
        let text = format!("{} {}", e.glyph(), e.name);
        f.buffer_mut()
            .set_string(x, y, clip(&text, w.saturating_sub(1) as usize), st);
        if vis == app.pane_at(idx).cursor {
            cursor_row(f.buffer_mut(), cell, active);
        }
    }
}

/// Column widths for Details. Name flexes; the rest are fixed like Dolphin's.
fn detail_columns(width: u16) -> [u16; 4] {
    let (size, modified, kind) = (12u16, 17u16, 14u16);
    let name = width.saturating_sub(size + modified + kind + 3).max(8);
    [name, size, modified, kind]
}

fn details_view(f: &mut Frame, app: &mut App, area: Rect, idx: usize, active: bool) {
    let row_h: u16 = 1;
    let head = area.y;
    let list = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    let rows = (list.height / row_h).max(1);
    let cols = detail_columns(area.width);
    let sort = app.pane_at(idx).sort;
    let cut = app.clipboard.cut;
    let cut_set = app.clipboard.paths.clone();

    {
        let p = app.pane_at_mut(idx);
        p.grid_cols = 1;
        p.grid_rows = rows;
        p.cell_w = area.width;
        p.cell_h = row_h;
        reveal(p, p.cursor, rows as usize);
    }

    // Header, clickable, with the sort arrow on the active column.
    let hst = Style::default().bg(color::TOOLBAR_BG).fg(color::DIM);
    fill(
        f.buffer_mut(),
        Rect::new(area.x, head, area.width, 1),
        color::TOOLBAR_BG,
    );
    app.hits.headers.clear();
    let keys = [SortKey::Name, SortKey::Size, SortKey::Date, SortKey::Type];
    let mut hx = area.x;
    for (i, key) in keys.iter().enumerate() {
        let arrow = if sort.key == *key {
            if sort.reverse {
                g::SORT_DESC
            } else {
                g::SORT_ASC
            }
        } else {
            " "
        };
        let label = format!("{}{}", key.label(), arrow);
        f.buffer_mut()
            .set_string(hx + 1, head, clip(&label, cols[i] as usize), hst);
        app.hits
            .headers
            .push((Rect::new(hx, head, cols[i], 1), *key));
        hx += cols[i] + 1;
    }

    let p_len = app.pane_at(idx).visible.len();
    let offset = app.pane_at(idx).offset;
    for r in 0..rows as usize {
        let vis = offset + r;
        if vis >= p_len {
            break;
        }
        let y = list.y + r as u16 * row_h;
        if y >= list.bottom() {
            break;
        }
        let e = {
            let p = app.pane_at(idx);
            p.entries[p.visible[vis]].clone()
        };
        let is_cut = cut && cut_set.contains(&e.path);
        let st = entry_style(app.pane_at(idx), vis, is_cut);
        let row = Rect::new(area.x, y, area.width, row_h.min(list.bottom() - y));
        if st.bg == Some(color::SELECTION) {
            fill(f.buffer_mut(), row, color::SELECTION);
        }

        // Expandable-folder arrow plus indent, Dolphin's tree column.
        let indent = e.depth * 2;
        let arrow = if e.is_dir() {
            if e.expanded {
                g::EXPAND_OPEN
            } else {
                g::EXPAND_CLOSED
            }
        } else {
            " "
        };
        let name = format!(
            "{}{} {} {}",
            " ".repeat(indent as usize),
            arrow,
            e.glyph(),
            e.name
        );
        let mut x = area.x;
        f.buffer_mut()
            .set_string(x + 1, y, clip(&name, cols[0] as usize), st);
        x += cols[0] + 1;
        f.buffer_mut().set_string(
            x,
            y,
            right(&fs::format_entry_size(&e), cols[1] as usize),
            st,
        );
        x += cols[1] + 1;
        f.buffer_mut()
            .set_string(x, y, clip(&fs::format_time(e.mtime), cols[2] as usize), st);
        x += cols[2] + 1;
        f.buffer_mut()
            .set_string(x, y, clip(&e.type_name(), cols[3] as usize), st);

        // With taller rows there is space for a real thumbnail beside the name.
        if row_h > 1 && e.is_image() {
            let ta = Rect::new(area.x + 1, y, (row_h * 2).min(cols[0]), row_h);
            if let Some(t) = app.thumbs.get(&e.path, ta.width, ta.height).cloned() {
                blit(f.buffer_mut(), ta, &t);
            }
        }

        if vis == app.pane_at(idx).cursor {
            cursor_row(f.buffer_mut(), row, active);
        }
    }
}

// ---------------------------------------------------------------------------
// Information panel (F11)
// ---------------------------------------------------------------------------

fn info_panel(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(color::PANEL_BG)),
        area,
    );
    let Some(e) = app.pane().current().cloned() else {
        return;
    };
    let preview = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 10);
    let drew = if e.is_image() {
        match app
            .thumbs
            .get(&e.path, preview.width, preview.height)
            .cloned()
        {
            Some(t) => {
                blit(f.buffer_mut(), preview, &t);
                true
            }
            None => false,
        }
    } else {
        false
    };
    if !drew {
        centred(
            f.buffer_mut(),
            preview,
            e.glyph(),
            Style::default().bg(color::PANEL_BG).fg(if e.is_locked() {
                color::OFFLINE
            } else if e.is_dir() {
                color::FOLDER
            } else {
                color::FILE
            }),
        );
    }
    let st = Style::default().bg(color::PANEL_BG).fg(color::TEXT);
    let dim = st.fg(color::DIM);
    let mut y = preview.bottom() + 1;
    let w = area.width.saturating_sub(2) as usize;
    let line = |f: &mut Frame, s: String, style: Style, y: &mut u16| {
        if *y < area.bottom() {
            f.buffer_mut()
                .set_string(area.x + 1, *y, clip(&s, w), style);
            *y += 1;
        }
    };
    line(f, e.name.clone(), st.add_modifier(Modifier::BOLD), &mut y);
    y += 1;
    line(f, format!("Type      {}", e.type_name()), dim, &mut y);
    line(
        f,
        format!("Size      {}", fs::format_entry_size(&e)),
        dim,
        &mut y,
    );
    line(
        f,
        format!("Modified  {}", fs::format_time(e.mtime)),
        dim,
        &mut y,
    );
    line(f, format!("Perms     {}", perms(e.mode)), dim, &mut y);
    line(f, format!("Path      {}", e.path.display()), dim, &mut y);
}

fn perms(mode: u32) -> String {
    let bit = |i: u32, c: char| if mode & (1 << i) != 0 { c } else { '-' };
    format!(
        "{}{}{}{}{}{}{}{}{}",
        bit(8, 'r'),
        bit(7, 'w'),
        bit(6, 'x'),
        bit(5, 'r'),
        bit(4, 'w'),
        bit(3, 'x'),
        bit(2, 'r'),
        bit(1, 'w'),
        bit(0, 'x'),
    )
}

// ---------------------------------------------------------------------------
// Filter bar and status bar
// ---------------------------------------------------------------------------

fn filter_bar(f: &mut Frame, app: &mut App, area: Rect) {
    fill(f.buffer_mut(), area, color::TOOLBAR_BG);
    let st = Style::default().bg(color::TOOLBAR_BG).fg(color::TEXT);
    let shown = if app.mode == Mode::Filter {
        &app.input
    } else {
        &app.pane().filter
    };
    let label = format!(" Filter: {shown}");
    f.buffer_mut()
        .set_string(area.x, area.y, clip(&label, area.width as usize), st);
    if app.mode == Mode::Filter {
        f.set_cursor_position((area.x + 9 + app.input_cursor as u16, area.y));
    }
}

fn status_bar(f: &mut Frame, app: &mut App, area: Rect) {
    fill(f.buffer_mut(), area, color::TOOLBAR_BG);
    let st = Style::default().bg(color::TOOLBAR_BG).fg(color::TEXT);

    // Command and search lines take over the status bar, as in vim.
    if matches!(app.mode, Mode::Command | Mode::Search) {
        let prefix = if app.mode == Mode::Command { ':' } else { '/' };
        let text = format!("{prefix}{}", app.input);
        f.buffer_mut()
            .set_string(area.x, area.y, clip(&text, area.width as usize), st);
        f.set_cursor_position((area.x + 1 + app.input_cursor as u16, area.y));
        return;
    }
    if let Mode::Rename(_) | Mode::BatchRename | Mode::NewFolder | Mode::NewFile = app.mode {
        let label = match app.mode {
            Mode::Rename(_) => "Rename to:",
            Mode::BatchRename => "Rename pattern (# = counter):",
            Mode::NewFolder => "New folder:",
            _ => "New file:",
        };
        let text = format!(" {label} {}", app.input);
        f.buffer_mut().set_string(
            area.x,
            area.y,
            clip(&text, area.width as usize),
            st.fg(color::ACCENT),
        );
        f.set_cursor_position((
            area.x + 2 + label.width() as u16 + app.input_cursor as u16,
            area.y,
        ));
        return;
    }

    let left = if !app.status.is_empty() {
        app.status.clone()
    } else {
        let p = app.pane();
        let (d, fi, bytes) = p.counts();
        let mut s = format!(
            "{d} folder{}, {fi} file{} ({})",
            plural(d),
            plural(fi),
            fs::format_size(bytes)
        );
        let sel = p.selected.len();
        if sel > 0 {
            s.push_str(&format!("   —   {sel} selected"));
        }
        s
    };
    let style = if app.status_is_error {
        st.fg(color::ERROR)
    } else {
        st
    };
    f.buffer_mut()
        .set_string(area.x + 1, area.y, clip(&left, area.width as usize), style);

    // Right side: free space, where stock Dolphin puts it.
    let free = match app.disk_space() {
        Some((avail, total)) => format!(
            "  {} free of {}",
            fs::format_size(avail),
            fs::format_size(total)
        ),
        None => String::new(),
    };
    let right_text = format!("{free} ");
    let rw = right_text.width() as u16;
    if rw < area.width {
        let x = area.right() - rw;
        f.buffer_mut()
            .set_string(x, area.y, &right_text, st.fg(color::DIM));
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

fn overlays(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(pr) = &app.progress {
        let r = centre_rect(area, 60, 6);
        let frac = pr.fraction();
        let cur = pr.current.lock().map(|g| g.clone()).unwrap_or_default();
        let body = vec![
            Line::from(pr.label.clone()),
            Line::from(Span::styled(cur, Style::default().fg(color::DIM))),
            Line::from(bar(frac, 56)),
            Line::from(Span::styled("Esc cancel", Style::default().fg(color::DIM))),
        ];
        popup(f, r, "Progress", body);
        return;
    }

    match app.mode.clone() {
        Mode::Menu(kind) => {
            let items = vim::menu_items(&kind);
            let w = items.iter().map(|(s, _)| s.width()).max().unwrap_or(20) as u16 + 4;
            let h = items.len() as u16 + 2;
            let anchor = match kind {
                MenuKind::ViewMode => app.hits.view_menu,
                _ => app.hits.menu,
            };
            let x = anchor.x.min(area.right().saturating_sub(w));
            // The popup hangs from the row under the toolbar, so the height it
            // may take is one less than the screen — not the whole of it.
            let r = Rect::new(x, area.y + 1, w, h.min(area.height.saturating_sub(1)));
            app.hits.menu_popup = inner_of(r);
            menu_popup(
                f,
                r,
                app.menu_sel,
                &items.iter().map(|(s, _)| s.to_string()).collect::<Vec<_>>(),
            );
        }
        Mode::CrumbMenu(seg) => {
            let items: Vec<String> = vim::crumb_siblings(app, seg)
                .iter()
                .map(|p| format!("{} {}", g::FOLDER, crate::ops::name(p)))
                .collect();
            if items.is_empty() {
                return;
            }
            let w = items.iter().map(|s| s.width()).max().unwrap_or(10) as u16 + 4;
            let h = (items.len() as u16 + 2).min(area.height.saturating_sub(1));
            let x = app
                .hits
                .crumb_arrows
                .iter()
                .find(|(_, i)| *i == seg)
                .map(|(r, _)| r.x)
                .unwrap_or(area.x);
            let r = Rect::new(x.min(area.right().saturating_sub(w)), area.y + 1, w, h);
            app.hits.menu_popup = inner_of(r);
            menu_popup(f, r, app.menu_sel, &items);
        }
        Mode::Confirm(c) => {
            let (title, text) = match &c {
                crate::app::Confirm::DeletePermanently(v) => (
                    "Delete permanently",
                    format!(
                        "Delete {} item(s) permanently? This cannot be undone.",
                        v.len()
                    ),
                ),
                crate::app::Confirm::PurgeFromTrash(v) => (
                    "Delete permanently",
                    format!(
                        "Delete {} item(s) from the Trash? This cannot be undone.",
                        v.len()
                    ),
                ),
                crate::app::Confirm::EmptyTrash => (
                    "Empty Trash",
                    "Permanently delete everything in the Trash?".to_string(),
                ),
            };
            let r = centre_rect(area, 60, 5);
            popup(
                f,
                r,
                title,
                vec![
                    Line::from(text),
                    Line::from(""),
                    Line::from(Span::styled(
                        "y / Enter  confirm      n / Esc  cancel",
                        Style::default().fg(color::DIM),
                    )),
                ],
            );
        }
        Mode::Properties => {
            let Some(e) = app.pane().current().cloned() else {
                return;
            };
            let r = centre_rect(area, 64, 12);
            let body = vec![
                Line::from(Span::styled(
                    e.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                kv("Type", &e.type_name()),
                kv("Size", &fs::format_entry_size(&e)),
                kv("Modified", &fs::format_time(e.mtime)),
                kv("Permissions", &perms(e.mode)),
                kv(
                    "Location",
                    &e.path
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                ),
                kv("Full path", &e.path.display().to_string()),
                Line::from(""),
                Line::from(Span::styled(
                    "any key to close",
                    Style::default().fg(color::DIM),
                )),
            ];
            popup(f, r, "Properties", body);
        }
        Mode::Help => {
            let r = centre_rect(area, 76, area.height.saturating_sub(4).min(30));
            popup(f, r, "Dolvim — keys", help_lines());
        }
        _ => {}
    }

    // The in-TUI drag badge: "N items" following the pointer.
    if let Some(d) = &app.drag {
        if d.started {
            let label = format!(" {} item{} ", d.paths.len(), plural(d.paths.len()));
            let w = label.width() as u16;
            let x = (d.at.0 + 1).min(area.right().saturating_sub(w));
            let y = (d.at.1 + 1).min(area.bottom().saturating_sub(1));
            f.buffer_mut().set_string(
                x,
                y,
                &label,
                Style::default().bg(color::ACCENT).fg(color::VIEW_BG),
            );
        }
    }
}

fn kv(k: &str, v: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<13}"), Style::default().fg(color::DIM)),
        Span::raw(v.to_string()),
    ])
}

fn help_lines() -> Vec<Line<'static>> {
    let sect = |s: &str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default()
                .fg(color::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<22}"), Style::default().fg(color::TEXT)),
            Span::styled(d.to_string(), Style::default().fg(color::DIM)),
        ])
    };
    vec![
        sect("Motion"),
        row("h j k l", "left / down / up / right (grid-aware)"),
        row("gg  G  5j  0  $", "top, bottom, counts, row start/end"),
        row("Ctrl+d / Ctrl+u", "half page"),
        row("Enter / l", "open        Backspace / h  up"),
        row("Alt+← / Alt+→", "back / forward in history"),
        sect("Selection"),
        row("Space  v  V", "toggle, visual, whole row"),
        row("Ctrl+A  Ctrl+Shift+A", "select all / invert"),
        sect("Files"),
        row("x  5x", "trash the item / n items"),
        row("dd  3dd  dj  dk  dG", "trash: this, n, or to a motion"),
        row("y  Ctrl+X  p", "copy, cut, paste"),
        row("r / cw / F2", "rename (batch when multi-selected)"),
        row("o  O / F10", "new file / new folder"),
        row("u", "undo        Shift+Del  delete forever"),
        row("D  P", "drag out / drop in (needs ripdrag)"),
        sect("View"),
        row("Ctrl+1/2/3", "icons / compact / details"),
        row("H", "toggle hidden files"),
        row("F3  F9  F11  Ctrl+I", "split, places, info, filter"),
        row("Ctrl+h / Ctrl+l", "focus the panel left / right"),
        sect("Toolbar row"),
        row("Ctrl+k / Ctrl+j", "up into the row, back down"),
        row("Ctrl+h  Ctrl+l", "nav buttons / trail / right buttons"),
        row("h  l", "previous / next item"),
        row("j  k / Ctrl+n  Ctrl+p", "down / up an open menu"),
        row("Ctrl+y  Enter  Tab", "accept          Esc  cancel"),
        row("F4", "shell here (suspends Dolvim)"),
        sect("Tabs and commands"),
        row("Ctrl+T Ctrl+W gt gT", "new, close, next, previous"),
        row(":e :cd :sort :view", ":split :q :qa"),
        row("/  n  N", "search        m  menu"),
    ]
}

fn bar(frac: f64, w: usize) -> String {
    let filled = (frac * w as f64).round() as usize;
    format!(
        "{}{}",
        "\u{2588}".repeat(filled.min(w)),
        "\u{2591}".repeat(w.saturating_sub(filled))
    )
}

fn popup(f: &mut Frame, r: Rect, title: &str, body: Vec<Line<'static>>) {
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "))
        .style(Style::default().bg(color::PANEL_BG).fg(color::TEXT))
        .border_style(Style::default().fg(color::ACCENT));
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(body), inner);
}

/// The clickable area inside a bordered popup.
fn inner_of(r: Rect) -> Rect {
    Rect::new(
        r.x + 1,
        r.y + 1,
        r.width.saturating_sub(2),
        r.height.saturating_sub(2),
    )
}

fn menu_popup(f: &mut Frame, r: Rect, sel: usize, items: &[String]) {
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(color::PANEL_BG).fg(color::TEXT))
        .border_style(Style::default().fg(color::SEPARATOR));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let buf = f.buffer_mut();
    // Scroll the menu when it is taller than the screen allows.
    let h = inner.height as usize;
    let first = sel.saturating_sub(h.saturating_sub(1));
    for (i, s) in items.iter().enumerate().skip(first).take(h) {
        let y = inner.y + (i - first) as u16;
        let st = if i == sel {
            Style::default().bg(color::SELECTION).fg(color::TEXT)
        } else {
            Style::default().bg(color::PANEL_BG).fg(color::TEXT)
        };
        fill(buf, Rect::new(inner.x, y, inner.width, 1), st.bg.unwrap());
        buf.set_string(
            inner.x + 1,
            y,
            clip(s, inner.width.saturating_sub(2) as usize),
            st,
        );
    }
}

// ---------------------------------------------------------------------------
// Buffer helpers
// ---------------------------------------------------------------------------

fn fill(buf: &mut Buffer, r: Rect, bg: ratatui::style::Color) {
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_char(' ').set_bg(bg);
            }
        }
    }
}

fn centred(buf: &mut Buffer, r: Rect, s: &str, st: Style) {
    if r.width == 0 || r.height == 0 {
        return;
    }
    let w = s.width().min(r.width as usize);
    let x = r.x + ((r.width as usize - w) / 2) as u16;
    let y = r.y + r.height / 2;
    buf.set_string(x, y, clip(s, r.width as usize), st);
}

/// Dolphin's focus rectangle: an outline, not a fill.
fn outline(buf: &mut Buffer, r: Rect, active: bool) {
    if r.width < 2 || r.height < 2 {
        cursor_row(buf, r, active);
        return;
    }
    let c = if active {
        color::ACCENT
    } else {
        color::SEPARATOR
    };
    let (l, t, rt, b) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
    for x in l..=rt {
        for y in [t, b] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.symbol() == " " {
                    cell.set_char('\u{2500}');
                }
                cell.set_fg(c);
            }
        }
    }
    for y in t..=b {
        for x in [l, rt] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.symbol() == " " {
                    cell.set_char('\u{2502}');
                }
                cell.set_fg(c);
            }
        }
    }
    for (x, y, ch) in [
        (l, t, '\u{256d}'),
        (rt, t, '\u{256e}'),
        (l, b, '\u{2570}'),
        (rt, b, '\u{256f}'),
    ] {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch).set_fg(c);
        }
    }
}

/// One-row cursor: a left accent bar, so it reads as focus without hiding text.
fn cursor_row(buf: &mut Buffer, r: Rect, active: bool) {
    let c = if active {
        color::ACCENT
    } else {
        color::SEPARATOR
    };
    for y in r.y..r.bottom() {
        if let Some(cell) = buf.cell_mut((r.x, y)) {
            cell.set_char('\u{2590}').set_fg(c);
        }
        if active {
            for x in r.x + 1..r.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_bg(color::SELECTION);
                }
            }
        }
    }
}

/// Paint a decoded thumbnail. Each cell is `▀`: fg is the top pixel row, bg
/// the bottom one — two pixels of vertical resolution per terminal cell.
fn blit(buf: &mut Buffer, r: Rect, t: &crate::thumbs::Thumb) {
    let ox = r.x + (r.width.saturating_sub(t.w)) / 2;
    let oy = r.y + (r.height.saturating_sub(t.h)) / 2;
    for cy in 0..t.h.min(r.height) {
        for cx in 0..t.w.min(r.width) {
            let (top, bot) = t.cells[(cy as usize) * t.w as usize + cx as usize];
            if let Some(c) = buf.cell_mut((ox + cx, oy + cy)) {
                c.set_char('\u{2580}')
                    .set_fg(ratatui::style::Color::Rgb(top[0], top[1], top[2]))
                    .set_bg(ratatui::style::Color::Rgb(bot[0], bot[1], bot[2]));
            }
        }
    }
}

/// Truncate to `w` display columns, with an ellipsis when it does not fit.
pub fn clip(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if s.width() <= w {
        return s.to_string();
    }
    format!("{}\u{2026}", cut(s, w - 1))
}

/// Truncate to `w` display columns, saying nothing about what was dropped.
fn cut(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.to_string().width();
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out
}

fn right(s: &str, w: usize) -> String {
    let s = clip(s, w);
    format!("{}{}", " ".repeat(w.saturating_sub(s.width())), s)
}

/// Wrap a name into at most `lines` rows of `w` columns, ellipsising the last.
pub fn wrap(s: &str, w: usize, lines: usize) -> Vec<String> {
    if w == 0 || lines == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.to_string().width();
        if used + cw > w {
            out.push(std::mem::take(&mut cur));
            used = 0;
            if out.len() == lines {
                break;
            }
        }
        cur.push(c);
        used += cw;
    }
    if out.len() < lines && !cur.is_empty() {
        out.push(cur);
    }
    // Whatever did not fit becomes an ellipsis on the final line.
    let consumed: usize = out.iter().map(|l| l.chars().count()).sum();
    if consumed < s.chars().count() {
        if let Some(last) = out.last_mut() {
            *last = clip(&format!("{last}\u{2026}"), w);
        }
    }
    out
}

/// Wrap a file name, keeping its extension on the final row.
///
/// Names in one folder collide in their middles far more often than at their
/// ends — twenty `WhatsApp Image 2023-…` differ only in the tail — and the
/// extension is the part that says what the file *is*. So the last row reads
/// `…xx.ext`: two characters of the stem, then the extension whole. When the
/// extension is too long to leave room for that, the plain ellipsis stands.
pub fn wrap_name(s: &str, w: usize, lines: usize) -> Vec<String> {
    let mut out = wrap(s, w, lines);
    if !out.last().is_some_and(|l| l.ends_with('\u{2026}')) {
        return out; // nothing was dropped
    }
    let Some((stem, ext)) = s.rsplit_once('.') else {
        return out;
    };
    if stem.is_empty() || ext.is_empty() {
        return out; // a dotfile: the leading dot is not an extension
    }
    let keep: String = stem
        .chars()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let tail = format!("\u{2026}{keep}.{ext}");
    if tail.width() > w {
        return out;
    }
    let last = out.last_mut().unwrap();
    *last = format!(
        "{}{tail}",
        cut(last.trim_end_matches('\u{2026}'), w - tail.width())
    );
    out
}

fn centre_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn clip_ellipsises_only_when_needed() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 4), "abc\u{2026}");
        assert_eq!(clip("abc", 0), "");
    }

    #[test]
    fn wrap_respects_the_line_budget() {
        let v = wrap("abcdefghij", 4, 2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], "abcd");
        assert!(v[1].ends_with('\u{2026}'));
    }

    #[test]
    fn a_truncated_name_keeps_its_extension() {
        let v = wrap_name("WhatsApp Image 2024-01-09 at 4.30.15 PM.jpeg", 13, 3);
        assert_eq!(v.len(), 3);
        assert_eq!(v[2], "at 4.\u{2026}PM.jpeg");
        // An extension too long to leave room falls back to a plain ellipsis.
        let v = wrap_name("sketch of the flashcards concept.excalidraw", 13, 3);
        assert!(v[2].ends_with('\u{2026}'));
        // A dotfile has no extension to protect.
        let v = wrap_name(&format!(".{}", "a".repeat(60)), 13, 3);
        assert!(v[2].ends_with('\u{2026}'));
    }

    #[test]
    fn detail_columns_always_leave_room_for_a_name() {
        for w in [10u16, 40, 80, 200] {
            let c = detail_columns(w);
            assert!(c[0] >= 8);
        }
    }

    #[test]
    fn crumbs_run_root_first() {
        let v = crumb_paths(Path::new("/a/b"));
        assert_eq!(v.first().unwrap(), Path::new("/"));
        assert_eq!(v.last().unwrap(), Path::new("/a/b"));
    }

    #[test]
    fn scroll_follows_the_cursor_both_ways() {
        let mut off = 0;
        scroll_to(&mut off, 12, 10);
        assert_eq!(off, 3);
        scroll_to(&mut off, 1, 10);
        assert_eq!(off, 1);
    }

    #[test]
    fn the_whole_window_renders_without_panicking() {
        let mut app = App::new(std::env::temp_dir());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        // The toolbar row must carry the Breeze toolbar background.
        assert_eq!(buf.cell((0, 0)).unwrap().bg, color::TOOLBAR_BG);
        // The status bar must report a count.
        let last: String = (0..100)
            .map(|x| buf.cell((x, 29)).unwrap().symbol().to_string())
            .collect();
        assert!(last.contains("folder"), "status bar was: {last}");
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = App::new(std::env::temp_dir());
        for (w, h) in [(1u16, 1u16), (5, 3), (20, 4)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
}
