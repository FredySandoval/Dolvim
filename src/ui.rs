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
use crate::config;
use crate::editor::Layout as EditorLayout;
use crate::fs::{self, SortKey};
use crate::ops::TransferProgress;
use crate::places::Row;
use crate::vim;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    paint(frame, area, config::THEME.view.background);
    if app.editor_layout() == Some(EditorLayout::Sidebar) {
        sidebar(frame, app, area);
        return;
    }

    let filter = if app.filter_bar { 1 } else { 0 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),      // toolbar + breadcrumb
            Constraint::Min(1),         // body (including the tab pane)
            Constraint::Length(filter), // filter bar
            Constraint::Length(1),      // status bar
        ])
        .split(area);

    toolbar(frame, app, rows[0]);
    body(frame, app, rows[1]);
    if filter == 1 {
        filter_bar(frame, app, rows[2]);
    }
    status_bar(frame, app, rows[3]);

    overlays(frame, app, area);
}

fn sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    app.hits = Default::default();
    let rows = area.height.max(1);
    let active = app
        .editor
        .as_ref()
        .is_some_and(|editor| editor.terminal_focused);
    {
        let pane = app.pane_mut();
        pane.area = area;
        pane.grid_cols = 1;
        pane.grid_rows = rows;
        pane.cell_width = area.width.max(1);
        pane.cell_height = 1;
        reveal_cursor(pane, pane.cursor, rows as usize);
    }
    let selected_path = app
        .editor
        .as_ref()
        .and_then(|editor| editor.selected_path.as_deref());
    let cut_paths = app.register.cut_paths();
    let pane = app.pane();
    let cut = !cut_paths.is_empty();
    let offset = pane.offset;
    for row in 0..rows as usize {
        let visible_index = offset + row;
        let Some(entry) = pane.entry_at(visible_index) else {
            break;
        };
        let y = area.y + row as u16;
        if y >= area.bottom() || area.width == 0 {
            break;
        }
        let selected = selected_path == Some(entry.path.as_path());
        let selection = if active {
            config::THEME.selection
        } else {
            config::THEME.inactive_selection
        };
        let style = if selected {
            Style::default()
                .fg(selection.foreground)
                .bg(selection.background)
        } else {
            let is_cut = cut && cut_paths.contains(&entry.path);
            entry_style(entry, entry_selected(pane, entry), is_cut, false)
        };
        let row_area = Rect::new(area.x, y, area.width, 1);
        if selected {
            fill(frame.buffer_mut(), row_area, selection.background);
        }
        let marker = if entry.is_dir() {
            if entry.expanded {
                config::glyph::EXPAND_OPEN
            } else {
                config::glyph::EXPAND_CLOSED
            }
        } else {
            " "
        };
        let text = format!(
            "{}{} {} {}",
            " ".repeat((entry.depth * 2) as usize),
            marker,
            entry.glyph(),
            entry.name
        );
        frame
            .buffer_mut()
            .set_string(area.x, y, clip(&text, area.width as usize), style);
        if visible_index == pane.cursor {
            let icon_x = area
                .x
                .saturating_add(entry.depth * 2)
                .saturating_add(marker.width().max(1) as u16 + 1)
                .min(area.right().saturating_sub(1));
            cursor_block(
                frame.buffer_mut(),
                Rect::new(icon_x, y, 1, 1),
                row_area,
                active,
                selected.then_some(row_area),
            );
        }
    }
    if app.pane().loading && app.pane().entries.is_empty() {
        centred(
            frame.buffer_mut(),
            area,
            "Loading…",
            Style::default().fg(config::THEME.view.secondary),
        );
    }
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
/// One drawn breadcrumb segment: its label, and where it sits in the full
/// trail (which elision from the left does not renumber).
struct CrumbSeg {
    path_index: usize,
    label: String,
}

fn crumb_label(p: &Path) -> String {
    let home = crate::places::home();
    if p == home {
        return "Home".into();
    }
    if p == Path::new("/") {
        return "/".into();
    }
    crate::ops::file_name_of(p)
}

fn toolbar(frame: &mut Frame, app: &mut App, area: Rect) {
    paint(frame, area, config::THEME.toolbar.background);
    // Where the text cursor should end up, applied once the buffer borrow ends.
    let mut cursor: Option<(u16, u16)> = None;
    let buf = frame.buffer_mut();
    let base = Style::default()
        .bg(config::THEME.toolbar.background)
        .fg(config::THEME.toolbar.foreground);
    let dim = base.fg(config::THEME.view.secondary);
    let p = app.pane();
    let can_back = p.history_pos > 0;
    let can_fwd = p.history_pos + 1 < p.history.len();

    // The focused toolbar button wears the same fill a selected file does. A
    // button whose menu is open is focused too — the open menu *is* the focus.
    let focused = match &app.mode {
        Mode::Buttons(i) => Some(*i),
        Mode::Menu(kind) => vim::menu_owner(kind),
        _ => None,
    };
    let focused_style = |n: usize, st: Style| match focused {
        Some(i) if i == n => st.bg(config::THEME.selection.background),
        _ => st,
    };

    let mut x = area.x;
    let put_left = |buf: &mut Buffer, text: &str, style: Style, x: &mut u16| -> Rect {
        let w = text.width() as u16;
        buf.set_string(*x, area.y, text, style);
        let r = Rect::new(*x, area.y, w, 1);
        *x += w;
        r
    };

    put_left(buf, " ", base, &mut x);
    let back_st = focused_style(0, if can_back { base } else { dim });
    app.hits.back = put_left(buf, config::glyph::BACK, back_st, &mut x);
    put_left(buf, "  ", base, &mut x);
    let fwd_st = focused_style(1, if can_fwd { base } else { dim });
    app.hits.forward = put_left(buf, config::glyph::FORWARD, fwd_st, &mut x);
    put_left(buf, "   ", base, &mut x);

    // Dolphin's split button: the icon shows the current mode and cycles it,
    // the caret beside it opens the full list. Two hitboxes, not one.
    let vg = match app.pane().view {
        ViewMode::Icons => config::glyph::VIEW_ICONS,
        ViewMode::Compact => config::glyph::VIEW_COMPACT,
        ViewMode::Details => config::glyph::VIEW_DETAILS,
    };
    let view_st = focused_style(2, base);
    app.hits.view_cycle = put_left(buf, vg, view_st, &mut x);
    put_left(buf, " ", view_st, &mut x);
    app.hits.view_menu = put_left(buf, config::glyph::DROPDOWN, view_st, &mut x);

    // The navigation group sits over the Places panel and the breadcrumb
    // starts where the file view does, which is how Dolphin lines them up.
    if app.places_visible {
        x = x.max(area.x + config::PLACES_WIDTH);
    }

    // Breadcrumb, or the editable path field when path edit is active.
    let crumb_area = Rect::new(
        x,
        area.y,
        area.width
            .saturating_sub(x - area.x)
            .saturating_sub(config::TOOLBAR_RIGHT_WIDTH),
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
            base.bg(config::THEME.view.background),
        );
        cursor = Some((crumb_area.x + 1 + app.input_cursor as u16, area.y));
    } else {
        let paths = crumb_paths(app.pane().display_path());
        // Dolphin elides from the left when the trail does not fit.
        let mut segs: Vec<CrumbSeg> = paths
            .iter()
            .enumerate()
            .map(|(path_index, p)| CrumbSeg {
                path_index,
                label: crumb_label(p),
            })
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
        let mut crumb_x = crumb_area.x;
        let last = segs.len().saturating_sub(1);
        for (draw_position, seg) in segs.iter().enumerate() {
            // One marker at the head of the trail, then blanks. A separator per
            // hop is three columns each of punctuation the path already implies.
            let sep = if draw_position == 0 {
                format!(" {} ", config::glyph::CRUMB_SEP)
            } else {
                "  ".to_string()
            };
            buf.set_string(crumb_x, area.y, &sep, dim);
            crumb_x += sep.width() as u16;
            let open = open_seg == Some(seg.path_index);
            let st = if open {
                base.bg(config::THEME.selection.background)
            } else if draw_position == last {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };
            let w = seg.label.width() as u16;
            if crumb_x + w > crumb_area.right() {
                break;
            }
            buf.set_string(crumb_x, area.y, &seg.label, st);
            app.hits.crumbs.push((
                Rect::new(crumb_x, area.y, w, 1),
                paths[seg.path_index].clone(),
            ));
            crumb_x += w;
            let arrow = Rect::new(crumb_x, area.y, 1, 1);
            let (glyph, arrow_style) = if open {
                (config::glyph::DROPDOWN, st)
            } else {
                (config::glyph::CRUMB_SHUT, dim)
            };
            buf.set_string(crumb_x, area.y, glyph, arrow_style);
            app.hits.crumb_arrows.push((arrow, seg.path_index));
            crumb_x += 1;
        }
    }

    // Right-aligned controls, laid out from the edge inward.
    let mut right_x = area.right();
    let put_right = |buf: &mut Buffer, text: &str, style: Style, right_x: &mut u16| -> Rect {
        let w = text.width() as u16;
        *right_x = right_x.saturating_sub(w);
        buf.set_string(*right_x, area.y, text, style);
        Rect::new(*right_x, area.y, w, 1)
    };
    put_right(buf, " ", base, &mut right_x);
    app.hits.menu = put_right(
        buf,
        config::glyph::MENU,
        focused_style(5, base),
        &mut right_x,
    );
    put_right(buf, "   ", base, &mut right_x);
    app.hits.search = put_right(
        buf,
        config::glyph::SEARCH,
        focused_style(4, base),
        &mut right_x,
    );
    put_right(buf, "   ", base, &mut right_x);
    let split_style = focused_style(
        3,
        if app.split_on() {
            base.fg(config::THEME.accent)
        } else {
            base
        },
    );
    app.hits.split = put_right(
        buf,
        &format!("{} Split", config::glyph::SPLIT),
        split_style,
        &mut right_x,
    );

    if let Some(pos) = cursor {
        frame.set_cursor_position(pos);
    }
}

/// Label plus arrow plus the blanks in front: three columns for the leading
/// ` › `, two for every hop after it.
fn total_crumb_width(segs: &[CrumbSeg]) -> u16 {
    segs.iter()
        .enumerate()
        .map(|(draw_position, seg)| {
            seg.label.width() as u16 + 1 + if draw_position == 0 { 3 } else { 2 }
        })
        .sum()
}

fn tab_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    paint(frame, area, config::THEME.panel.background);
    let buf = frame.buffer_mut();
    app.hits.tabs.clear();
    let mut x = area.x;
    for (i, t) in app.tabs.iter().enumerate() {
        let label = format!(" {} ", t.title());
        let w = label.width() as u16;
        if x + w > area.right() {
            break;
        }
        let st = if i == app.active_tab {
            Style::default()
                .bg(if app.focus == Focus::Tabs {
                    config::THEME.selection.background
                } else {
                    config::THEME.view.background
                })
                .fg(config::THEME.view.foreground)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(config::THEME.panel.background)
                .fg(config::THEME.view.secondary)
        };
        buf.set_string(x, area.y, &label, st);
        app.hits.tabs.push(Rect::new(x, area.y, w, 1));
        x += w;
    }
}

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

fn body(frame: &mut Frame, app: &mut App, area: Rect) {
    let places_width = if app.places_visible {
        config::PLACES_WIDTH
    } else {
        0
    };
    let info_width = if app.info_visible {
        config::INFO_WIDTH
    } else {
        0
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(places_width),
            Constraint::Min(10),
            Constraint::Length(info_width),
        ])
        .split(area);

    if places_width > 0 {
        places_panel(frame, app, cols[0]);
    }
    app.hits.places = cols[0];

    // Tabs are a pane above the file views, not a window-wide strip. This keeps
    // the Places heading in the same row and gives both regions their own column.
    let tabbar = if app.tabs.len() > 1 { 1 } else { 0 };
    let view_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(tabbar), Constraint::Min(1)])
        .split(cols[1]);
    if tabbar == 1 {
        tab_bar(frame, app, view_rows[0]);
    } else {
        app.hits.tabs.clear();
    }
    let view_area = view_rows[1];
    app.hits.headers.clear();

    let n = app.tab().panes.len();
    if n > 1 {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(view_area);
        for i in 0..n {
            draw_pane(frame, app, halves[i], i);
        }
        // The divider between the two views.
        let x = halves[1].x.saturating_sub(1);
        for y in view_area.y..view_area.bottom() {
            if let Some(c) = frame.buffer_mut().cell_mut((x, y)) {
                c.set_char('\u{2502}').set_fg(config::THEME.separator);
            }
        }
    } else {
        draw_pane(frame, app, view_area, 0);
    }

    if info_width > 0 {
        info_panel(frame, app, cols[2]);
    }
}

fn places_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    paint(frame, area, config::THEME.panel.background);
    let focused = vim::current_focus_region(app) == crate::app::FocusRegion::Places;
    let buf = frame.buffer_mut();
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
                    Style::default()
                        .bg(config::THEME.panel.background)
                        .fg(config::THEME.view.secondary),
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
                let selected = i == app.places_cursor;
                let selection = if focused {
                    config::THEME.selection
                } else {
                    config::THEME.inactive_selection
                };
                let bg = if selected {
                    selection.background
                } else {
                    config::THEME.panel.background
                };
                let fg = if selected {
                    selection.foreground
                } else {
                    config::THEME.view.foreground
                };
                let st = Style::default().bg(bg).fg(fg);
                for x in area.x..area.x + w {
                    if let Some(c) = buf.cell_mut((x, y)) {
                        c.set_char(' ').set_style(st);
                    }
                }
                // Devices carry a free-space gauge behind the label.
                if let Some(usage) = gauge {
                    if usage.total_bytes > 0 {
                        let filled = ((usage.used_bytes as f64 / usage.total_bytes as f64)
                            * w as f64) as u16;
                        for x in area.x..area.x + filled.min(w) {
                            if let Some(c) = buf.cell_mut((x, y)) {
                                c.set_bg(if selected {
                                    selection.background
                                } else {
                                    config::THEME.gauge_full
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
                    config::THEME.entry.unreachable
                } else {
                    config::THEME.view.foreground
                };
                buf.set_string(area.x + 1, y, glyph, Style::default().fg(fg));
                let gw = glyph.width().max(1) as u16 + 1;
                // The eject affordance sits at the right edge, so the label
                // has to give up its columns before it is clipped, not after.
                let ew = if *eject {
                    config::glyph::EJECT.width().max(1) as u16 + 1
                } else {
                    0
                };
                buf.set_string(
                    area.x + 1 + gw,
                    y,
                    clip(label, w.saturating_sub(2 + gw + ew) as usize),
                    Style::default().fg(fg),
                );
                if *eject {
                    buf.set_string(
                        area.x + w.saturating_sub(ew),
                        y,
                        config::glyph::EJECT,
                        Style::default().fg(config::THEME.view.secondary),
                    );
                }
                // The row already carries the selection fill; only the block is
                // new, and only while the panel has focus.
                if selected && focused {
                    cursor_block(
                        buf,
                        icon_cell(area.x + 1, y, glyph),
                        Rect::new(area.x, y, w, 1),
                        true,
                        Some(Rect::new(area.x, y, w, 1)),
                    );
                }
                if selected {
                    state_marker(
                        buf,
                        Rect::new(area.x, y, w, 1),
                        if focused { '>' } else { '*' },
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// File views
// ---------------------------------------------------------------------------

fn draw_pane(frame: &mut Frame, app: &mut App, area: Rect, pane_index: usize) {
    let is_active = vim::current_focus_region(app) == crate::app::FocusRegion::View(pane_index);
    paint(frame, area, config::THEME.view.background);
    {
        let p = app.pane_at_mut(pane_index);
        p.area = area;
    }

    let mode = app.pane_at(pane_index).view;
    match mode {
        ViewMode::Icons => draw_icons_view(frame, app, area, pane_index, is_active),
        ViewMode::Compact => draw_compact_view(frame, app, area, pane_index, is_active),
        ViewMode::Details => draw_details_view(frame, app, area, pane_index, is_active),
    }

    let p = app.pane_at(pane_index);
    if p.loading && p.entries.is_empty() {
        centred(
            frame.buffer_mut(),
            area,
            "Loading…",
            Style::default()
                .fg(config::THEME.view.secondary)
                .bg(config::THEME.view.background),
        );
    } else if p.entries.is_empty() && p.error.is_some() {
        let msg = p.error.clone().unwrap_or_default();
        centred(
            frame.buffer_mut(),
            area,
            &msg,
            Style::default()
                .fg(config::THEME.error)
                .bg(config::THEME.view.background),
        );
    } else if p.is_empty() {
        let msg = if p.filter.is_empty() {
            "This folder is empty."
        } else {
            "No items match the filter."
        };
        centred(
            frame.buffer_mut(),
            area,
            msg,
            Style::default()
                .fg(config::THEME.view.secondary)
                .bg(config::THEME.view.background),
        );
    }
}

fn entry_selected(pane: &Pane, entry: &fs::Entry) -> bool {
    pane.selected.contains(entry.selection_key())
}

fn entry_style(e: &fs::Entry, selected: bool, is_cut: bool, active: bool) -> Style {
    let fg = if is_cut {
        config::THEME.entry.cut
    } else if e.is_locked() {
        config::THEME.entry.unreachable
    } else if e.is_dir() {
        config::THEME.entry.folder
    } else if e.kind == fs::Kind::Symlink {
        config::THEME.entry.symlink
    } else if e.is_executable() {
        config::THEME.entry.executable
    } else {
        config::THEME.view.foreground
    };
    if selected {
        let selection = if active {
            config::THEME.selection
        } else {
            config::THEME.inactive_selection
        };
        Style::default()
            .fg(selection.foreground)
            .bg(selection.background)
    } else {
        Style::default().fg(fg).bg(config::THEME.view.background)
    }
}

/// Scroll to the cursor, but only when the cursor is what moved.
///
/// Dolphin's wheel scrolls the view and leaves the selection alone, even off
/// screen, so the renderer must not drag the viewport back on the next frame.
/// A view change is in the key, since it reorients the axis the cursor sits on.
fn reveal_cursor(pane: &mut Pane, cursor_line: usize, visible_lines: usize) {
    let state = (pane.cursor, pane.view);
    if pane.last_reveal == state {
        return;
    }
    pane.last_reveal = state;
    scroll_to(&mut pane.offset, cursor_line, visible_lines);
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
pub struct IconGrid {
    pub cols: u16,
    pub rows: u16,
    /// Left margin that centres the grid in the pane.
    pub margin_x: u16,
}

pub fn icon_grid(area: Rect) -> IconGrid {
    let (cell_width, cell_height) = (config::CELL_WIDTH, config::CELL_HEIGHT);
    let cols = (area.width.saturating_sub(2) / cell_width).max(1);
    let rows = (area.height.saturating_sub(1) / cell_height).max(1);
    // Columns never divide the pane evenly. The remainder becomes margin, split
    // between the two sides, so the grid stays centred as the pane is resized.
    IconGrid {
        cols,
        rows,
        margin_x: area.width.saturating_sub(cols * cell_width) / 2,
    }
}

fn draw_icons_view(frame: &mut Frame, app: &mut App, area: Rect, idx: usize, active: bool) {
    let (cell_width, cell_height) = (config::CELL_WIDTH, config::CELL_HEIGHT);
    let IconGrid {
        cols,
        rows,
        margin_x,
    } = icon_grid(area);
    {
        let p = app.pane_at_mut(idx);
        p.grid_cols = cols;
        p.grid_rows = rows;
        p.cell_width = cell_width;
        p.cell_height = cell_height;
        p.grid_x = area.x + margin_x;
        let cur_row = p.cursor / cols as usize;
        reveal_cursor(p, cur_row, rows as usize);
    }
    let active_tab = app.active_tab;
    let cut_paths = app.register.cut_paths();
    let cut = !cut_paths.is_empty();
    let p = &app.tabs[active_tab].panes[idx];
    let thumbs = &mut app.thumbs;
    let first = p.offset * cols as usize;

    let gap = config::CELL_GAP.min(cell_width.saturating_sub(3));
    let tile_w = cell_width - gap;
    let body_h = cell_height - 1;
    // The name always gets its rows; the icon takes what is left.
    let name_lines = config::NAME_LINES.clamp(1, body_h.saturating_sub(1).max(1));
    let icon_h = body_h.saturating_sub(name_lines);

    for slot in 0..(cols as usize * rows as usize) {
        let vis = first + slot;
        if vis >= p.visible.len() {
            break;
        }
        let (r, c) = (slot / cols as usize, slot % cols as usize);
        let x0 = area.x + margin_x + c as u16 * cell_width;
        let y0 = area.y + r as u16 * cell_height;
        let cell = Rect::new(
            x0,
            y0,
            cell_width.min(area.right().saturating_sub(x0)),
            cell_height.min(area.bottom().saturating_sub(y0)),
        );
        if cell.width == 0 || cell.height == 0 {
            continue;
        }
        let e = &p.entries[p.visible[vis]];
        let is_cut = cut && cut_paths.contains(&e.path);
        let selected = entry_selected(p, e);
        let st = entry_style(e, selected, is_cut, active);

        // Selection fill covers the whole cell.
        if selected {
            let selection = if active {
                config::THEME.selection
            } else {
                config::THEME.inactive_selection
            };
            fill(frame.buffer_mut(), cell, selection.background);
        }

        // Thumbnail, or the glyph stand-in while one is being decoded.
        let body = Rect::new(
            cell.x + gap / 2,
            cell.y + 1,
            tile_w.min(cell.width.saturating_sub(gap / 2)),
            cell.height.saturating_sub(1),
        );
        let thumb_area = Rect::new(body.x, body.y, body.width, icon_h.min(body.height));
        let drew = e.is_image()
            && thumb_area.width > 1
            && thumb_area.height > 0
            && try_draw_thumbnail(frame, thumbs, thumb_area, &e.path);
        if !drew {
            centred(
                frame.buffer_mut(),
                thumb_area,
                e.glyph(),
                st.fg(icon_color(e, is_cut)),
            );
        }

        // Name, wrapped over its rows and centred, as Dolphin centres it.
        let name = wrap_name(&e.name, body.width as usize, name_lines as usize);
        for (li, part) in name.iter().enumerate() {
            let y = body.y + icon_h + li as u16;
            if y >= body.bottom() {
                break;
            }
            let x = body.x + body.width.saturating_sub(part.width() as u16) / 2;
            frame.buffer_mut().set_string(x, y, part, st);
        }

        if vis == p.cursor {
            // The frame hugs what is actually drawn: the blank margin row it is
            // hung from, the icon, and only the name rows this name used. A
            // one-line name gets a short box instead of two rows of empty space,
            // and its bottom edge falls on a row the cell was not using anyway.
            let used = (name.len() as u16).max(1);
            let h = (icon_h + used + 2).min(area.bottom().saturating_sub(y0));
            outline(
                frame.buffer_mut(),
                Rect::new(cell.x, cell.y, cell.width, h),
                active,
            );
        }
        if selected || (active && vis == p.cursor) {
            state_marker(
                frame.buffer_mut(),
                cell,
                if active && vis == p.cursor { '>' } else { '*' },
            );
        }
    }
}

fn draw_compact_view(frame: &mut Frame, app: &mut App, area: Rect, idx: usize, active: bool) {
    let rows = area.height.max(1);
    // The margin is the pane's, not each column's: columns are already held
    // apart by the blank `compact_widths` leaves on the right.
    let right = area.right();
    let grid_x = area
        .x
        .saturating_add(config::VIEW_MARGIN.min(area.width))
        .min(right);
    let avail = right.saturating_sub(grid_x);
    {
        let p = app.pane_at_mut(idx);
        p.grid_rows = rows;
        p.cell_height = 1;
        p.grid_x = grid_x;
        p.column_widths.clear();
        if avail == 0 {
            p.grid_cols = 1;
            p.cell_width = 0;
            return;
        }

        ensure_compact_widths(p, rows, avail);
        // Compact flows down columns, so the scroll axis is columns.
        let cur_col = p.cursor / rows as usize;
        if p.last_reveal != (p.cursor, p.view) {
            p.last_reveal = (p.cursor, p.view);
            scroll_columns(&mut p.offset, cur_col, &p.compact_widths, avail);
        }

        // Only whole columns are drawn, except the first: at any width something
        // must be on screen, even if it is a name too long for the pane.
        let mut used = 0u16;
        for &width in p.compact_widths.iter().skip(p.offset) {
            if width > avail.saturating_sub(used) && !p.column_widths.is_empty() {
                break;
            }
            let width = width.min(avail.saturating_sub(used));
            p.column_widths.push(width);
            used = used.saturating_add(width);
        }
        p.grid_cols = p.column_widths.len().max(1) as u16;
        p.cell_width = p.column_widths.first().copied().unwrap_or(1);
    }

    let active_tab = app.active_tab;
    let cut_paths = app.register.cut_paths();
    let cut = !cut_paths.is_empty();
    let p = &app.tabs[active_tab].panes[idx];
    let mut x = grid_x;
    for (c, &column_width) in p.column_widths.iter().enumerate() {
        for r in 0..rows {
            let vis = (p.offset + c) * rows as usize + r as usize;
            if vis >= p.visible.len() {
                break;
            }
            let y = area.y + r;
            if y >= area.bottom() {
                break;
            }
            let e = &p.entries[p.visible[vis]];
            let is_cut = cut && cut_paths.contains(&e.path);
            let selected = entry_selected(p, e);
            let st = entry_style(e, selected, is_cut, active);
            let w = column_width.min(right.saturating_sub(x));
            let cell = Rect::new(x, y, w, 1);
            if selected {
                let selection = if active {
                    config::THEME.selection
                } else {
                    config::THEME.inactive_selection
                };
                fill(frame.buffer_mut(), cell, selection.background);
            }
            let text = compact_entry_text(e);
            frame
                .buffer_mut()
                .set_string(x, y, clip(&text, w.saturating_sub(1) as usize), st);
            if vis == p.cursor {
                let icon = icon_cell(x, y, e.glyph());
                cursor_block(
                    frame.buffer_mut(),
                    icon,
                    cell,
                    active,
                    selected.then_some(cell),
                );
            }
            if selected || (active && vis == p.cursor) {
                state_marker(
                    frame.buffer_mut(),
                    cell,
                    if active && vis == p.cursor { '>' } else { '*' },
                );
            }
        }
        x = x.saturating_add(column_width).min(right);
    }
}

/// Dolphin's Compact sizes every column to its own longest name rather than
/// truncating to a fixed cell, so a column of short names stays narrow and a
/// long one is read in full. Widths for *all* columns, scrolled or not: which
/// column an item lands in depends only on the row count, so this does not
/// shift as the view scrolls.
fn compact_entry_text(e: &fs::Entry) -> String {
    format!(
        "{}{} {}",
        " ".repeat((e.depth * 2) as usize),
        e.glyph(),
        e.name
    )
}

fn ensure_compact_widths(p: &mut crate::app::Pane, rows: u16, avail: u16) {
    if !p.compact_widths.is_empty()
        && p.compact_width_rows == rows
        && p.compact_width_avail == avail
        && p.compact_width_generation == p.content_generation
    {
        return;
    }
    p.compact_width_rows = rows;
    p.compact_width_avail = avail;
    p.compact_width_generation = p.content_generation;
    p.compact_widths.clear();
    p.compact_widths
        .extend(p.visible.chunks(rows as usize).map(|col| {
            let max_item_width = col
                .iter()
                .map(|&i| {
                    let entry = &p.entries[i];
                    entry.depth as usize * 2 + entry.glyph().width() + 1 + entry.name.width()
                })
                .max()
                .unwrap_or(1);
            // One trailing blank keeps neighbouring columns from touching.
            ((max_item_width + 1) as u16).min(avail.max(1))
        }));
}

/// Scroll the least that brings column `col` on screen. Ragged widths mean the
/// number that fits depends on where you start, so this walks left from `col`
/// rather than subtracting a column count.
fn scroll_columns(offset: &mut usize, col: usize, widths: &[u16], avail: u16) {
    if col < *offset {
        *offset = col;
        return;
    }
    let Some(&cw) = widths.get(col) else { return };
    let mut used = cw;
    let mut start = col;
    while start > *offset && used + widths[start - 1] <= avail {
        used += widths[start - 1];
        start -= 1;
    }
    if start > *offset {
        *offset = start;
    }
}

/// One Details column: where it sits and how its text is placed in it.
///
/// Header, click target and cell are all drawn from this one value. The
/// alternative — widths here, x positions worked out again at each draw site —
/// is what let the headings drift off their own data.
#[derive(Clone, Copy)]
struct Col {
    x: u16,
    width: u16,
    /// Numeric columns right-align, and their heading right-aligns with them.
    right_aligned: bool,
}

impl Col {
    fn draw_text(self, buf: &mut Buffer, y: u16, s: &str, st: Style) {
        let s = clip(s, self.width as usize);
        let x = if self.right_aligned {
            self.x + self.width - s.width() as u16
        } else {
            self.x
        };
        buf.set_string(x, y, s, st);
    }
}

/// Blank columns between one column and the next.
const DETAIL_GAP: u16 = 1;

/// What a rendered year costs: a space and four digits.
const YEAR_W: u16 = 5;

/// What the Modified column needs for this listing. `Short` prints the year
/// only outside the current one, so a directory touched entirely this year
/// wants `YEAR_W` fewer columns — and handing them to Name instead is what
/// Compact already does with its own widths. The year is the only part that
/// varies in width, so this scans integers rather than formatted strings.
fn time_width(p: &crate::app::Pane) -> u16 {
    let this_year = fs::year_of(fs::now_epoch());
    let dated = |e: &fs::Entry| fs::year_of(e.mtime) != this_year;
    if config::TIME_STYLE == fs::TimeStyle::Iso || p.entries.iter().any(dated) {
        config::TIME_WIDTH
    } else {
        config::TIME_WIDTH - YEAR_W
    }
}

/// Column geometry for Details. Name flexes; the rest are fixed like Dolphin's.
/// The row's left margin is the first column's offset, not something every
/// caller remembers to add.
///
/// `modified_width` is passed rather than taken from config because the width a
/// timestamp needs depends on the listing — see `time_width`.
fn detail_columns(area: Rect, modified_width: u16) -> [Col; 4] {
    let margin = config::VIEW_MARGIN.min(area.width);
    let gap = if area.width.saturating_sub(margin) >= 3 {
        DETAIL_GAP
    } else {
        0
    };
    let available = area.width.saturating_sub(margin + 3 * gap);
    let preferred = [8, config::SIZE_WIDTH, modified_width, config::TYPE_WIDTH];
    let mut widths = [0; 4];

    // Details may share a narrow body with a second pane and the information
    // panel. Every column must therefore be derived from the pane's rectangle;
    // forcing Name to eight cells after allocating the fixed columns lets the
    // row escape its pane and overwrite its neighbours.
    let mut remaining = available;
    for (width, minimum) in widths.iter_mut().zip([8, 1, 1, 1]) {
        *width = minimum.min(remaining);
        remaining -= *width;
    }
    for i in [1, 2, 3, 0] {
        let extra = remaining.min(preferred[i].saturating_sub(widths[i]));
        widths[i] += extra;
        remaining -= extra;
    }
    widths[0] += remaining;

    let mut cols = [Col {
        x: 0,
        width: 0,
        right_aligned: false,
    }; 4];
    let mut x = area.x + margin;
    for (col, (width, right_aligned)) in cols
        .iter_mut()
        .zip(widths.into_iter().zip([false, true, true, false]))
    {
        *col = Col {
            x,
            width,
            right_aligned,
        };
        x += width + gap;
    }
    cols
}

fn draw_details_view(frame: &mut Frame, app: &mut App, area: Rect, idx: usize, active: bool) {
    // One item per row, always: Details is Dolphin's list, not a grid.
    let head = area.y;
    let list = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    let rows = list.height.max(1);
    let cols = detail_columns(area, time_width(app.pane_at(idx)));
    let sort = app.pane_at(idx).sort;

    {
        let p = app.pane_at_mut(idx);
        p.grid_cols = 1;
        p.grid_rows = rows;
        p.cell_width = area.width;
        p.cell_height = 1;
        reveal_cursor(p, p.cursor, rows as usize);
    }

    // Header, clickable, with the sort arrow on the active column.
    let hst = Style::default()
        .bg(config::THEME.toolbar.background)
        .fg(config::THEME.view.secondary);
    fill(
        frame.buffer_mut(),
        Rect::new(area.x, head, area.width, 1),
        config::THEME.toolbar.background,
    );
    let keys = [SortKey::Name, SortKey::Size, SortKey::Date, SortKey::Type];
    // A click anywhere up to the next column sorts by this one: the gaps belong
    // to the column on their left, so no cell of the header row is dead.
    let mut hx = area.x;
    for (col, key) in cols.iter().zip(keys) {
        let arrow = if sort.key == key {
            if sort.reverse {
                config::glyph::SORT_DESC
            } else {
                config::glyph::SORT_ASC
            }
        } else {
            " "
        };
        col.draw_text(
            frame.buffer_mut(),
            head,
            &format!("{}{}", key.label(), arrow),
            hst,
        );
        let end = col.x + col.width;
        app.hits
            .headers
            .push((Rect::new(hx, head, end - hx, 1), idx, key));
        hx = end;
    }

    let active_tab = app.active_tab;
    let cut_paths = app.register.cut_paths();
    let cut = !cut_paths.is_empty();
    let p = &app.tabs[active_tab].panes[idx];
    for r in 0..rows as usize {
        let vis = p.offset + r;
        if vis >= p.visible.len() {
            break;
        }
        let y = list.y + r as u16;
        if y >= list.bottom() {
            break;
        }
        let e = &p.entries[p.visible[vis]];
        let is_cut = cut && cut_paths.contains(&e.path);
        let selected = entry_selected(p, e);
        let st = entry_style(e, selected, is_cut, active);
        let row = Rect::new(area.x, y, area.width, 1);
        if selected {
            let selection = if active {
                config::THEME.selection
            } else {
                config::THEME.inactive_selection
            };
            fill(frame.buffer_mut(), row, selection.background);
        }

        // Expandable-folder arrow plus indent, Dolphin's tree column.
        let indent = e.depth * 2;
        let arrow = if e.is_dir() {
            if e.expanded {
                config::glyph::EXPAND_OPEN
            } else {
                config::glyph::EXPAND_CLOSED
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
        cols[0].draw_text(frame.buffer_mut(), y, &name, st);
        cols[1].draw_text(frame.buffer_mut(), y, &fs::format_entry_size(e), st);
        cols[2].draw_text(frame.buffer_mut(), y, &fs::format_time(e.mtime), st);
        cols[3].draw_text(frame.buffer_mut(), y, &e.type_name(), st);

        if vis == p.cursor {
            // The tree column pushes the icon right: indent, arrow, one blank.
            let ix = cols[0].x + indent + arrow.width().max(1) as u16 + 1;
            cursor_block(
                frame.buffer_mut(),
                icon_cell(ix, y, e.glyph()),
                row,
                active,
                selected.then_some(row),
            );
        }
        if selected || (active && vis == p.cursor) {
            state_marker(
                frame.buffer_mut(),
                row,
                if active && vis == p.cursor { '>' } else { '*' },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Information panel (F11)
// ---------------------------------------------------------------------------

fn info_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    paint(frame, area, config::THEME.panel.background);
    let Some(e) = app.pane().current().cloned() else {
        return;
    };
    let preview = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 10);
    if !(e.is_image() && try_draw_thumbnail(frame, &mut app.thumbs, preview, &e.path)) {
        centred(
            frame.buffer_mut(),
            preview,
            e.glyph(),
            Style::default()
                .bg(config::THEME.panel.background)
                .fg(icon_color(&e, false)),
        );
    }
    let st = Style::default()
        .bg(config::THEME.panel.background)
        .fg(config::THEME.view.foreground);
    let dim = st.fg(config::THEME.view.secondary);
    let mut y = preview.bottom() + 1;
    let w = area.width.saturating_sub(2) as usize;
    let draw_info_line = |frame: &mut Frame, s: String, style: Style, y: &mut u16| {
        if *y < area.bottom() {
            frame
                .buffer_mut()
                .set_string(area.x + 1, *y, clip(&s, w), style);
            *y += 1;
        }
    };
    draw_info_line(
        frame,
        e.name.clone(),
        st.add_modifier(Modifier::BOLD),
        &mut y,
    );
    y += 1;
    draw_info_line(frame, format!("Type      {}", e.type_name()), dim, &mut y);
    draw_info_line(
        frame,
        format!("Size      {}", fs::format_entry_size(&e)),
        dim,
        &mut y,
    );
    draw_info_line(
        frame,
        format!("Modified  {}", fs::format_time(e.mtime)),
        dim,
        &mut y,
    );
    draw_info_line(frame, format!("Perms     {}", perms(e.mode)), dim, &mut y);
    draw_info_line(
        frame,
        format!("Path      {}", e.path.display()),
        dim,
        &mut y,
    );
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

fn filter_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    fill(frame.buffer_mut(), area, config::THEME.toolbar.background);
    let st = Style::default()
        .bg(config::THEME.toolbar.background)
        .fg(config::THEME.toolbar.foreground);
    let shown = if app.mode == Mode::Filter {
        &app.input
    } else {
        &app.pane().filter
    };
    let label = format!(" Filter: {shown}");
    frame
        .buffer_mut()
        .set_string(area.x, area.y, clip(&label, area.width as usize), st);
    if app.mode == Mode::Filter {
        frame.set_cursor_position((area.x + 9 + app.input_cursor as u16, area.y));
    }
}

fn status_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    // On a one-row terminal the layout hands us a zero-height rect whose `y` is
    // already past the buffer. Nothing here may touch it.
    if area.height == 0 || area.width == 0 {
        return;
    }
    fill(frame.buffer_mut(), area, config::THEME.toolbar.background);
    let st = Style::default()
        .bg(config::THEME.toolbar.background)
        .fg(config::THEME.toolbar.foreground);

    // Command and search lines take over the status bar, as in vim.
    if matches!(app.mode, Mode::Command | Mode::Search) {
        let prefix = if app.mode == Mode::Command { ':' } else { '/' };
        let text = format!("{prefix}{}", app.input);
        frame
            .buffer_mut()
            .set_string(area.x, area.y, clip(&text, area.width as usize), st);
        frame.set_cursor_position((area.x + 1 + app.input_cursor as u16, area.y));
        return;
    }
    if let Mode::Rename(_) | Mode::BatchRename | Mode::NewFolder(_) | Mode::NewFile(_) = app.mode {
        let label = match app.mode {
            Mode::Rename(_) => "Rename to:",
            Mode::BatchRename => "Rename pattern (# = counter):",
            Mode::NewFolder(_) => "New folder:",
            _ => "New file:",
        };
        let text = format!(" {label} {}", app.input);
        frame.buffer_mut().set_string(
            area.x,
            area.y,
            clip(&text, area.width as usize),
            st.fg(config::THEME.accent),
        );
        frame.set_cursor_position((
            area.x + 2 + label.width() as u16 + app.input_cursor as u16,
            area.y,
        ));
        return;
    }

    // The mode name leads the row, as vim's `showmode` does. It sits in its own
    // cell rather than inside the message slot: a transient status must never
    // be able to hide which mode owns the keyboard.
    let tag = format!(" -- {} --  ", app.mode.name());
    let tw = (tag.width() as u16).min(area.width);
    frame.buffer_mut().set_string(
        area.x,
        area.y,
        clip(&tag, area.width as usize),
        st.add_modifier(Modifier::BOLD),
    );

    let left = if !app.status.is_empty() {
        app.status.clone()
    } else {
        let p = app.pane();
        let counts = p.counts();
        let mut s = format!(
            "{} folder{}, {} file{} ({})",
            counts.dirs,
            plural(counts.dirs),
            counts.files,
            plural(counts.files),
            fs::format_size(counts.bytes)
        );
        let selected_count = p.selected.len();
        if selected_count > 0 {
            s.push_str(&format!("   —   {selected_count} selected"));
        }
        s
    };
    let style = if app.status_is_error {
        st.fg(config::THEME.error)
    } else {
        st
    };
    frame.buffer_mut().set_string(
        area.x + tw,
        area.y,
        clip(&left, area.width.saturating_sub(tw) as usize),
        style,
    );

    // Right side: the incomplete command sits immediately before the free
    // space, where stock Dolphin puts it. `showcmd` owns the slot in Normal
    // mode only; a pending count, operator, chord leader or mark name shows
    // here in high contrast (bold, not the grayed free-space style) until the
    // command completes or Esc clears it. The two are separate segments so the
    // free-space readout stays dim even while a command is pending.
    let pending = if app.mode == Mode::Normal {
        app.pending_command()
    } else {
        String::new()
    };
    let free = match app.disk_space() {
        Some(space) => format!(
            "  {} free of {}",
            fs::format_size(space.available_bytes),
            fs::format_size(space.total_bytes)
        ),
        None => String::new(),
    };
    let avail = usize::from(area.width);
    let pending = clip_start(&pending, avail);
    let pending_w = pending.width();
    let free_w = free.width();
    // The free-space readout yields rather than hiding a pending command.
    if !pending.is_empty() && pending_w <= avail {
        if free_w > 0 && pending_w.saturating_add(free_w).saturating_add(1) <= avail {
            let free_x = area.right() - free_w as u16 - 1;
            frame.buffer_mut().set_string(
                free_x,
                area.y,
                &free,
                st.fg(config::THEME.view.secondary),
            );
            frame.buffer_mut().set_string(
                free_x - pending_w as u16,
                area.y,
                &pending,
                st.add_modifier(Modifier::BOLD),
            );
        } else {
            frame.buffer_mut().set_string(
                area.right() - pending_w as u16,
                area.y,
                &pending,
                st.add_modifier(Modifier::BOLD),
            );
        }
    } else if free_w > 0 && free_w.saturating_add(1) <= avail {
        frame.buffer_mut().set_string(
            area.right() - free_w as u16 - 1,
            area.y,
            &free,
            st.fg(config::THEME.view.secondary),
        );
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

fn overlays(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(active_transfer) = &app.active_transfer {
        let r = centre_rect(area, config::PROGRESS_POPUP_W, config::PROGRESS_POPUP_H);
        let snapshot = active_transfer.progress.snapshot();
        let (current_entry, mut progress) = transfer_progress_display(&snapshot);
        if active_transfer.progress.is_cancelling() {
            progress = format!("Cancelling… {progress}");
        }
        let body = vec![
            Line::from(active_transfer.progress.label.clone()),
            Line::from(Span::styled(
                current_entry,
                Style::default().fg(config::THEME.view.secondary),
            )),
            Line::from(progress),
            Line::from(Span::styled(
                "Esc cancel",
                Style::default().fg(config::THEME.view.secondary),
            )),
        ];
        popup(frame, r, "Progress", body);
        return;
    }

    match app.mode.clone() {
        Mode::Menu(kind) => {
            let items = vim::menu_items(&kind);
            let w = items
                .iter()
                .map(|item| item.label.width())
                .max()
                .unwrap_or(20) as u16
                + 4;
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
                frame,
                r,
                app.menu_cursor,
                &items
                    .iter()
                    .map(|item| item.label.to_string())
                    .collect::<Vec<_>>(),
            );
        }
        Mode::CrumbMenu(segment_index) => {
            let items: Vec<String> = vim::crumb_siblings(app, segment_index)
                .iter()
                .map(|p| format!("{} {}", config::glyph::FOLDER, crate::ops::file_name_of(p)))
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
                .find(|(_, i)| *i == segment_index)
                .map(|(r, _)| r.x)
                .unwrap_or(area.x);
            let r = Rect::new(x.min(area.right().saturating_sub(w)), area.y + 1, w, h);
            app.hits.menu_popup = inner_of(r);
            menu_popup(frame, r, app.menu_cursor, &items);
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
                frame,
                r,
                title,
                vec![
                    Line::from(text),
                    Line::from(""),
                    Line::from(Span::styled(
                        "y / Enter  confirm      n / Esc  cancel",
                        Style::default().fg(config::THEME.view.secondary),
                    )),
                ],
            );
        }
        Mode::Properties => {
            let Some(e) = app.pane().current().cloned() else {
                return;
            };
            let r = centre_rect(area, 64, 12 + u16::from(e.link_target.is_some()));
            let mut body = vec![
                Line::from(Span::styled(
                    e.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                labelled_row("Type", &e.type_name()),
            ];
            if let Some(target) = &e.link_target {
                body.push(labelled_row("Link target", &target.display().to_string()));
            }
            body.extend([
                labelled_row("Size", &fs::format_entry_size(&e)),
                labelled_row("Modified", &fs::format_time(e.mtime)),
                labelled_row("Permissions", &perms(e.mode)),
                labelled_row(
                    if e.backing_path.is_some() || e.trash_id().is_some() {
                        "Original location"
                    } else {
                        "Location"
                    },
                    &e.path
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                ),
                labelled_row(
                    if e.backing_path.is_some() || e.trash_id().is_some() {
                        "Original path"
                    } else {
                        "Full path"
                    },
                    &e.path.display().to_string(),
                ),
                Line::from(""),
                Line::from(Span::styled(
                    "Esc / q to close",
                    Style::default().fg(config::THEME.view.secondary),
                )),
            ]);
            popup(frame, r, "Properties", body);
        }
        Mode::Help => {
            let r = centre_rect(area, 76, area.height.saturating_sub(4).min(30));
            popup(frame, r, "Dolvim — keys", help_lines());
        }
        _ => {}
    }

    // The in-TUI drag badge: "N items" following the pointer.
    if let Some(d) = &app.drag {
        if d.started {
            let label = format!(" {} item{} ", d.paths.len(), plural(d.paths.len()));
            let w = label.width() as u16;
            let x = (d.position.x + 1).min(area.right().saturating_sub(w));
            let y = (d.position.y + 1).min(area.bottom().saturating_sub(1));
            frame.buffer_mut().set_string(
                x,
                y,
                &label,
                Style::default()
                    .bg(config::THEME.accent)
                    .fg(config::THEME.active_cursor.foreground),
            );
        }
    }
}

fn labelled_row(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<13}"),
            Style::default().fg(config::THEME.view.secondary),
        ),
        Span::raw(value.to_string()),
    ])
}

fn help_lines() -> Vec<Line<'static>> {
    let section_heading = |s: &str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default()
                .fg(config::THEME.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let key_row = |keys: &str, description: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {keys:<22}"),
                Style::default().fg(config::THEME.view.foreground),
            ),
            Span::styled(
                description.to_string(),
                Style::default().fg(config::THEME.view.secondary),
            ),
        ])
    };
    vec![
        section_heading("Motion"),
        key_row("h j k l", "left / down / up / right (grid-aware)"),
        key_row("gg  G  5j  0  $", "top, bottom, counts, row start/end"),
        key_row("Ctrl+d / Ctrl+u", "half page"),
        key_row("Enter", "open the selected folder or file"),
        key_row("H / L", "history back / forward (H falls back to up)"),
        key_row("Alt+← / Alt+→", "back / forward in history"),
        section_heading("Selection"),
        key_row("v  V  Ctrl+V", "visual, by row, block (Icons only)"),
        key_row("y", "yank visual selection"),
        key_row("Ctrl+A  Ctrl+Shift+A", "select all / invert"),
        section_heading("Files"),
        key_row("x  5x", "trash the item / n items"),
        key_row("dd  3dd  dj  dk  dG", "trash: this, n, or to a motion"),
        key_row("Ctrl+C  Ctrl+X  p", "copy, cut, paste"),
        key_row("cw / F2", "rename (batch when multi-selected)"),
        key_row("o  O / F10", "new file / new folder"),
        key_row("u", "undo        Shift+Del  delete forever"),
        section_heading("View"),
        key_row("Ctrl+1/2/3", "icons / compact / details"),
        key_row("<Space>h", "toggle hidden files"),
        key_row("F3  F9  F11  Ctrl+I", "split, places, info, filter"),
        key_row("Ctrl+h / Ctrl+l", "focus the panel left / right"),
        section_heading("Tabs and toolbar rows"),
        key_row("Ctrl+k / Ctrl+j", "focus the pane above / below"),
        key_row("h / l in tabs", "previous / next tab"),
        key_row("Ctrl+h/l in tabs", "focus left/right file view"),
        key_row("Ctrl+h/l in toolbar", "nav buttons / trail / right buttons"),
        key_row("h  l", "previous / next item; a menu button opens"),
        key_row("j  k / Ctrl+n  Ctrl+p", "down / up an open menu"),
        key_row("Ctrl+y  Enter  Tab", "accept          Esc  cancel"),
        key_row("F4 / Shift+F4", "shell here / separate terminal"),
        section_heading("Tabs and commands"),
        key_row("J / K", "previous / next tab"),
        key_row("Ctrl+T Ctrl+W gt gT", "new, close, next, previous"),
        key_row(":e :cd :sort :view", ":split :q :qa"),
        key_row("/  n  N", "search      Ctrl+k then the menu button"),
        key_row("ma  'a", "mark this item / go back to it"),
    ]
}

fn transfer_progress_display(progress: &TransferProgress) -> (String, String) {
    match progress {
        TransferProgress::Discovering {
            completed_entries,
            current_entry,
        } => (
            current_entry.clone(),
            format!("Working\u{2026} {completed_entries} entries complete"),
        ),
        TransferProgress::Items {
            completed_items,
            total_items,
            current_entry,
        } => {
            let complete = completed_items >= total_items;
            let fraction = if *total_items == 0 {
                1.0
            } else {
                (*completed_items as f64 / *total_items as f64).clamp(0.0, 1.0)
            };
            let percentage = if complete {
                100
            } else {
                ((fraction * 100.0).floor() as u8).min(99)
            };
            (
                current_entry.clone(),
                format!(
                    "{} {:>3}%",
                    progress_bar(fraction, config::PROGRESS_BAR_WIDTH),
                    percentage
                ),
            )
        }
        TransferProgress::Cleanup {
            retained_outputs,
            cleanup_failures,
            current_entry,
        } => (
            current_entry.clone(),
            format!(
                "Cleaning up… {retained_outputs} retained, {cleanup_failures} cleanup failures"
            ),
        ),
    }
}

fn progress_bar(fraction: f64, width: usize) -> String {
    let filled = (fraction * width as f64).floor() as usize;
    format!(
        "{}{}",
        "\u{2588}".repeat(filled.min(width)),
        "\u{2591}".repeat(width.saturating_sub(filled))
    )
}

fn popup(frame: &mut Frame, r: Rect, title: &str, body: Vec<Line<'static>>) {
    frame.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "))
        .style(
            Style::default()
                .bg(config::THEME.panel.background)
                .fg(config::THEME.panel.foreground),
        )
        .border_style(Style::default().fg(config::THEME.accent));
    let inner = block.inner(r);
    frame.render_widget(block, r);
    frame.render_widget(Paragraph::new(body), inner);
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

fn menu_popup(frame: &mut Frame, r: Rect, selected_row: usize, items: &[String]) {
    frame.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(
            Style::default()
                .bg(config::THEME.panel.background)
                .fg(config::THEME.panel.foreground),
        )
        .border_style(Style::default().fg(config::THEME.separator));
    let inner = block.inner(r);
    frame.render_widget(block, r);
    let buf = frame.buffer_mut();
    // Scroll the menu when it is taller than the screen allows.
    let h = inner.height as usize;
    let first = selected_row.saturating_sub(h.saturating_sub(1));
    for (i, s) in items.iter().enumerate().skip(first).take(h) {
        let y = inner.y + (i - first) as u16;
        let st = if i == selected_row {
            Style::default()
                .bg(config::THEME.selection.background)
                .fg(config::THEME.selection.foreground)
        } else {
            Style::default()
                .bg(config::THEME.panel.background)
                .fg(config::THEME.panel.foreground)
        };
        fill(buf, Rect::new(inner.x, y, inner.width, 1), st.bg.unwrap());
        buf.set_string(
            inner.x + 1,
            y,
            clip(s, inner.width.saturating_sub(2) as usize),
            st,
        );
        if i == selected_row {
            state_marker(buf, Rect::new(inner.x, y, inner.width, 1), '>');
        }
    }
}

// ---------------------------------------------------------------------------
// Buffer helpers
// ---------------------------------------------------------------------------

/// Lay a background colour under a whole region, so the gaps between widgets
/// are Breeze, not terminal default.
fn paint(frame: &mut Frame, r: Rect, bg: ratatui::style::Color) {
    frame.render_widget(Block::default().style(Style::default().bg(bg)), r);
}

fn fill(buf: &mut Buffer, r: Rect, bg: ratatui::style::Color) {
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_char(' ').set_bg(bg);
            }
        }
    }
}

fn state_marker(buf: &mut Buffer, area: Rect, marker: char) {
    if let Some(cell) = buf.cell_mut((area.x, area.y)) {
        let background = cell.bg;
        cell.set_char(marker).set_fg(background);
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
fn outline(buf: &mut Buffer, rect: Rect, active: bool) {
    if rect.width < 2 || rect.height < 2 {
        cursor_block(buf, Rect::new(rect.x, rect.y, 1, 1), rect, active, None);
        return;
    }
    let c = if active {
        config::THEME.accent
    } else {
        config::THEME.separator
    };
    let (left, top, right, bottom) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
    for x in left..=right {
        for y in [top, bottom] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.symbol() == " " {
                    cell.set_char('\u{2500}');
                }
                cell.set_fg(c);
            }
        }
    }
    for y in top..=bottom {
        for x in [left, right] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.symbol() == " " {
                    cell.set_char('\u{2502}');
                }
                cell.set_fg(c);
            }
        }
    }
    for (x, y, ch) in [
        (left, top, '\u{256d}'),
        (right, top, '\u{256e}'),
        (left, bottom, '\u{2570}'),
        (right, bottom, '\u{256f}'),
    ] {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch).set_fg(c);
        }
    }
}

/// The cursor is the terminal's own: a block sitting *on* the entry's icon with
/// its colours inverted. A bar in the margin had to borrow a column and read as
/// a fifth kind of line; the icon is already there and already means "this one".
/// An unselected cursor row gets the configurable hover fill. Selection wins
/// where the two overlap, while the accent icon still identifies the cursor.
fn cursor_block(buf: &mut Buffer, icon: Rect, row: Rect, active: bool, selection: Option<Rect>) {
    if active {
        for y in row.y..row.bottom() {
            for x in row.x..row.right() {
                let selected = selection.is_some_and(|area| {
                    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
                });
                if !selected {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(config::THEME.hover.background);
                    }
                }
            }
        }
    }
    // ACCENT identifies the active cursor; the unfocused pane gets DIM.
    let cursor = if active {
        config::THEME.active_cursor
    } else {
        config::THEME.inactive_cursor
    };
    for y in icon.y..icon.bottom() {
        for x in icon.x..icon.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(cursor.background).set_fg(cursor.foreground);
            }
        }
    }
}

/// Where the icon sits within a row, given the columns drawn before it.
fn icon_cell(x: u16, y: u16, glyph: &str) -> Rect {
    Rect::new(x, y, glyph.width().max(1) as u16, 1)
}

/// The icon's colour says what the entry *is*, not whether it is selected.
fn icon_color(e: &fs::Entry, cut: bool) -> ratatui::style::Color {
    if cut {
        config::THEME.entry.cut
    } else if e.is_locked() {
        config::THEME.entry.unreachable
    } else if e.is_dir() {
        config::THEME.entry.folder
    } else {
        config::THEME.entry.file
    }
}

/// Draw `path`'s thumbnail into `r`, requesting a decode if there is none yet.
/// False means "nothing to draw" and the caller falls back to the glyph; the
/// thumbnail pops in on a later frame, like Dolphin.
fn try_draw_thumbnail(
    frame: &mut Frame,
    thumbs: &mut crate::thumbs::Thumbs,
    rect: Rect,
    path: &Path,
) -> bool {
    match thumbs
        .get_or_request(path, rect.width, rect.height)
        .cloned()
    {
        Some(thumb) => {
            draw_thumbnail(frame.buffer_mut(), rect, &thumb);
            true
        }
        None => false,
    }
}

/// Paint a decoded thumbnail. Each cell is `▀`: fg is the top pixel row, bg
/// the bottom one — two pixels of vertical resolution per terminal cell.
fn draw_thumbnail(buf: &mut Buffer, rect: Rect, thumb: &crate::thumbs::Thumb) {
    let origin_x = rect.x + (rect.width.saturating_sub(thumb.cell_width)) / 2;
    let origin_y = rect.y + (rect.height.saturating_sub(thumb.cell_height)) / 2;
    for cell_y in 0..thumb.cell_height.min(rect.height) {
        for cell_x in 0..thumb.cell_width.min(rect.width) {
            let (top, bot) =
                thumb.cells[(cell_y as usize) * thumb.cell_width as usize + cell_x as usize];
            if let Some(c) = buf.cell_mut((origin_x + cell_x, origin_y + cell_y)) {
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
    format!("{}\u{2026}", truncate_to_width(s, w - 1))
}

/// Keep the end of a string visible, marking a dropped prefix with an ellipsis.
fn clip_start(s: &str, w: usize) -> String {
    if w == 0 || s.width() <= w {
        return truncate_to_width(s, w);
    }
    if w == 1 {
        return s.chars().next_back().into_iter().collect();
    }
    let mut used = 1;
    let mut chars = Vec::new();
    for c in s.chars().rev() {
        let cw = c.to_string().width();
        if used + cw > w {
            break;
        }
        chars.push(c);
        used += cw;
    }
    chars.reverse();
    format!("\u{2026}{}", chars.into_iter().collect::<String>())
}

/// Truncate to `w` display columns, saying nothing about what was dropped.
fn truncate_to_width(s: &str, w: usize) -> String {
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
        truncate_to_width(last.trim_end_matches('\u{2026}'), w - tail.width())
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

    #[test]
    fn discovery_progress_is_explicitly_indeterminate() {
        let (entry, display) = transfer_progress_display(&TransferProgress::Discovering {
            completed_entries: 7,
            current_entry: "folder".into(),
        });

        assert_eq!(entry, "folder");
        assert_eq!(display, "Working\u{2026} 7 entries complete");
        assert!(!display.contains('%'));
        assert!(!display.contains('\u{2588}'));
    }

    #[test]
    fn stable_item_progress_has_a_determinate_percentage() {
        let (entry, display) = transfer_progress_display(&TransferProgress::Items {
            completed_items: 1,
            total_items: 4,
            current_entry: "note.txt".into(),
        });

        assert_eq!(entry, "note.txt");
        assert!(display.ends_with(" 25%"));
        assert_eq!(
            display
                .chars()
                .filter(|character| *character == '\u{2588}')
                .count(),
            config::PROGRESS_BAR_WIDTH / 4
        );
    }
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn clip_ellipsises_only_when_needed() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 4), "abc\u{2026}");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(clip_start("abcdef", 4), "\u{2026}def");
        assert_eq!(clip_start("abc", 0), "");
    }

    #[test]
    fn wrap_respects_the_line_budget() {
        let wrapped_lines = wrap("abcdefghij", 4, 2);
        assert_eq!(wrapped_lines.len(), 2);
        assert_eq!(wrapped_lines[0], "abcd");
        assert!(wrapped_lines[1].ends_with('\u{2026}'));
    }

    #[test]
    fn a_truncated_name_keeps_its_extension() {
        let wrapped_lines = wrap_name("WhatsApp Image 2024-01-09 at 4.30.15 PM.jpeg", 13, 3);
        assert_eq!(wrapped_lines.len(), 3);
        assert_eq!(wrapped_lines[2], "at 4.\u{2026}PM.jpeg");
        // An extension too long to leave room falls back to a plain ellipsis.
        let wrapped_lines = wrap_name("sketch of the flashcards concept.excalidraw", 13, 3);
        assert!(wrapped_lines[2].ends_with('\u{2026}'));
        // A dotfile has no extension to protect.
        let wrapped_lines = wrap_name(&format!(".{}", "a".repeat(60)), 13, 3);
        assert!(wrapped_lines[2].ends_with('\u{2026}'));
    }

    #[test]
    fn detail_columns_never_escape_their_pane() {
        for width in 0..100 {
            let area = Rect::new(7, 0, width, 10);
            let columns = detail_columns(area, config::TIME_WIDTH);
            let right = columns
                .last()
                .map(|column| column.x + column.width)
                .unwrap();
            assert!(
                right <= area.right(),
                "width {width} escaped by {}",
                right - area.right()
            );
        }
    }

    fn render_test_entry(name: &str) -> fs::Entry {
        fs::Entry {
            name: name.into(),
            path: PathBuf::from("/tmp").join(name),
            backing_path: None,
            link_target: None,
            kind: fs::Kind::File,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            trash_identity: None,
            depth: 0,
            expanded: false,
        }
    }

    fn compact_app_with_entry() -> App {
        let mut app = App::new(std::env::temp_dir());
        let pane = app.pane_mut();
        pane.view = ViewMode::Compact;
        pane.entries = vec![render_test_entry("entry.txt")];
        pane.visible = vec![0];
        pane.loading = false;
        app
    }

    #[test]
    fn nonempty_compact_view_accepts_zero_width() {
        let mut app = compact_app_with_entry();
        let mut terminal = Terminal::new(TestBackend::new(2, 2)).unwrap();

        terminal
            .draw(|frame| draw_compact_view(frame, &mut app, Rect::new(1, 0, 0, 2), 0, true))
            .unwrap();

        assert_eq!(app.pane().grid_x, 1);
        assert_eq!(app.pane().cell_width, 0);
        assert!(app.pane().column_widths.is_empty());
    }

    #[test]
    fn nonempty_compact_view_clamps_tiny_columns_to_the_pane() {
        let mut app = compact_app_with_entry();
        let area = Rect::new(1, 0, 2, 2);
        let mut terminal = Terminal::new(TestBackend::new(area.right(), 2)).unwrap();

        terminal
            .draw(|frame| draw_compact_view(frame, &mut app, area, 0, true))
            .unwrap();

        let pane = app.pane();
        assert_eq!(pane.grid_x, 2);
        assert_eq!(pane.column_widths, vec![1]);
        assert!(pane.grid_x + pane.column_widths.iter().copied().sum::<u16>() <= area.right());
    }

    #[test]
    fn every_file_view_renders_trash_selection_and_cut_highlighting() {
        for view in [ViewMode::Icons, ViewMode::Compact, ViewMode::Details] {
            let mut app = App::new(std::env::temp_dir());
            let mut selected = render_test_entry("selected.txt");
            selected.set_trash_id("generation-1");
            let cut = render_test_entry("cut.txt");
            let selected_key = selected.selection_key().to_path_buf();
            let cut_path = cut.path.clone();
            {
                let pane = app.pane_mut();
                pane.view = view;
                pane.entries = vec![selected, cut];
                pane.visible = vec![0, 1];
                pane.selected.insert(selected_key);
                pane.loading = false;
            }
            app.register.set(vec![cut_path], true);

            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let pane_area = app.pane().area;
            let buffer = terminal.backend().buffer();
            let cells = pane_area.rows().flat_map(|row| row.columns());
            let styles: Vec<_> = cells.filter_map(|position| buffer.cell(position)).collect();

            assert!(
                styles.iter().any(|cell| {
                    cell.bg == config::THEME.selection.background
                        || cell.bg == config::THEME.inactive_selection.background
                }),
                "{view:?} did not render the Trash generation selection"
            );
            assert!(
                styles.iter().any(|cell| cell.fg == config::THEME.entry.cut),
                "{view:?} did not render the cut entry"
            );
        }
    }

    #[test]
    fn compact_entries_include_their_tree_depth() {
        let entry = fs::Entry {
            name: "child.txt".into(),
            path: PathBuf::from("parent/child.txt"),
            backing_path: None,
            link_target: None,
            kind: fs::Kind::File,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            trash_identity: None,
            depth: 2,
            expanded: false,
        };
        let text = compact_entry_text(&entry);
        assert!(text.starts_with("    "), "compact entry was: {text:?}");
        assert!(text.ends_with("child.txt"));
    }

    #[test]
    fn cursor_hover_does_not_hide_selection() {
        assert_ne!(
            config::THEME.hover.background,
            config::THEME.selection.background
        );
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        buf.cell_mut((0, 0))
            .unwrap()
            .set_bg(config::THEME.view.background);
        buf.cell_mut((1, 0))
            .unwrap()
            .set_bg(config::THEME.selection.background);
        cursor_block(
            &mut buf,
            Rect::new(2, 0, 1, 1),
            area,
            true,
            Some(Rect::new(1, 0, 1, 1)),
        );

        assert_eq!(buf.cell((0, 0)).unwrap().bg, config::THEME.hover.background);
        assert_eq!(
            buf.cell((1, 0)).unwrap().bg,
            config::THEME.selection.background
        );
        assert_eq!(buf.cell((2, 0)).unwrap().bg, config::THEME.accent);
    }

    #[test]
    fn detail_columns_tile_the_pane_without_overrunning_it() {
        for pane_width in [10u16, 40, 80, 200] {
            let area = Rect::new(3, 0, pane_width, 1);
            let columns = detail_columns(area, config::TIME_WIDTH);
            assert_eq!(columns[0].x, area.x + config::VIEW_MARGIN.min(pane_width));
            for pair in columns.windows(2) {
                assert!(pair[1].x >= pair[0].x + pair[0].width);
            }
            assert!(columns[3].x + columns[3].width <= area.right());
        }
    }

    #[test]
    fn crumbs_run_root_first() {
        let crumbs = crumb_paths(Path::new("/a/b"));
        assert_eq!(crumbs.first().unwrap(), Path::new("/"));
        assert_eq!(crumbs.last().unwrap(), Path::new("/a/b"));
    }

    #[test]
    fn scroll_follows_the_cursor_both_ways() {
        let mut offset = 0;
        scroll_to(&mut offset, 12, 10);
        assert_eq!(offset, 3);
        scroll_to(&mut offset, 1, 10);
        assert_eq!(offset, 1);
    }

    #[test]
    fn the_whole_window_renders_without_panicking() {
        let mut app = App::new(std::env::temp_dir());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|frame| draw(frame, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        // The toolbar row must carry the Breeze toolbar background.
        assert_eq!(
            buf.cell((0, 0)).unwrap().bg,
            config::THEME.toolbar.background
        );
        // The status bar must report a count.
        let last: String = (0..100)
            .map(|x| buf.cell((x, 29)).unwrap().symbol().to_string())
            .collect();
        assert!(last.contains("folder"), "status bar was: {last}");
    }

    /// `showcmd`: a pending command renders at the right of the status line,
    /// before the disk-free readout, in high contrast (bold) rather than the
    /// grayed free-space style.
    #[test]
    fn pending_command_renders_before_disk_free_in_high_contrast() {
        let mut app = App::new(std::env::temp_dir());
        crate::vim::handle_key_event(
            &mut app,
            ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Char('5'),
                ratatui::crossterm::event::KeyModifiers::NONE,
            ),
        );
        crate::vim::handle_key_event(
            &mut app,
            ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Char('z'),
                ratatui::crossterm::event::KeyModifiers::NONE,
            ),
        );
        assert_eq!(app.pending_command(), "5z");

        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|frame| draw(frame, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        let last: String = (0..100)
            .map(|x| buf.cell((x, 29)).unwrap().symbol().to_string())
            .collect();
        let pos = last.find("5z");
        assert!(pos.is_some(), "status bar was: {last}");
        // The pending command sits flush against the left edge of the grayed
        // free-space segment: `5z` is two cells wide and its last cell is the
        // one immediately before the dim readout starts.
        let pending_end = pos.unwrap() + 2;
        let dim_start = (0..100)
            .position(|x| buf.cell((x, 29)).unwrap().fg == config::THEME.view.secondary)
            .expect("free-space readout must stay grayed");
        assert_eq!(
            pending_end, dim_start,
            "pending must abut the free-space readout, got: {last}"
        );
        // The pending text is bold (high contrast) and never grayed; the
        // free-space text is the opposite, so the two readouts stay distinct.
        let pending_cell = buf.cell((pos.unwrap() as u16, 29)).unwrap();
        let free_cell = buf.cell((dim_start as u16, 29)).unwrap();
        assert!(
            pending_cell.modifier.contains(Modifier::BOLD)
                && pending_cell.fg != config::THEME.view.secondary,
            "pending command must be high contrast, got: {last}"
        );
        assert!(
            free_cell.fg == config::THEME.view.secondary
                && !free_cell.modifier.contains(Modifier::BOLD),
            "free-space readout must stay grayed, got: {last}"
        );
    }

    /// `showcmd` keeps the pending command at the right edge even on a row too
    /// narrow to hold it alongside the free-space readout, which yields.
    #[test]
    fn narrow_row_keeps_pending_and_drops_free_space() {
        let mut app = App::new(std::env::temp_dir());
        crate::vim::handle_key_event(
            &mut app,
            ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Char('5'),
                ratatui::crossterm::event::KeyModifiers::NONE,
            ),
        );
        crate::vim::handle_key_event(
            &mut app,
            ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Char('z'),
                ratatui::crossterm::event::KeyModifiers::NONE,
            ),
        );
        assert_eq!(app.pending_command(), "5z");

        // Width 12 holds `5z` but not "  X free of Y".
        let mut term = Terminal::new(TestBackend::new(12, 30)).unwrap();
        term.draw(|frame| draw(frame, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        let pending: String = (8..12)
            .map(|x| buf.cell((x, 29)).unwrap().symbol())
            .collect();
        assert!(
            pending.ends_with("5z"),
            "pending must survive a narrow row: {pending}"
        );
        // The free-space readout is dropped rather than squeezing the command.
        assert!(
            (0..12).all(|x| buf.cell((x, 29)).unwrap().fg != config::THEME.view.secondary),
            "free space must yield on a narrow row"
        );

        app.count = "123456789012345".into();
        term.draw(|frame| draw(frame, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let clipped: String = (0..12)
            .map(|x| buf.cell((x, 29)).unwrap().symbol())
            .collect();
        assert!(
            clipped.starts_with('\u{2026}') && clipped.ends_with('z'),
            "a long command must keep its distinguishing suffix: {clipped}"
        );
    }

    #[test]
    fn tabs_share_the_places_heading_row_and_own_the_view_column() {
        let mut app = App::new(std::env::temp_dir());
        app.tabs
            .push(crate::app::Tab::new(std::env::temp_dir().join("other")));
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|frame| draw(frame, &mut app)).unwrap();

        assert_eq!(app.hits.tabs[0].x, config::PLACES_WIDTH);
        assert_eq!(app.hits.tabs[0].y, 1);
        assert_eq!(app.pane().area.y, 2);
        let buf = term.backend().buffer();
        let places: String = (0..config::PLACES_WIDTH)
            .map(|x| buf.cell((x, 1)).unwrap().symbol())
            .collect();
        assert!(places.contains("Places"));
    }

    #[test]
    fn hiding_the_tab_pane_clears_hits_and_returns_the_view_row() {
        let mut app = App::new(std::env::temp_dir());
        app.tabs
            .push(crate::app::Tab::new(std::env::temp_dir().join("other")));
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(!app.hits.tabs.is_empty());

        app.tabs.truncate(1);
        term.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(app.hits.tabs.is_empty());
        assert_eq!(app.pane().area.y, 1);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = App::new(std::env::temp_dir());
        for (w, h) in [(1u16, 1u16), (5, 3), (20, 4)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|frame| draw(frame, &mut app)).unwrap();
        }
    }

    #[test]
    fn sidebar_renders_without_chrome_at_tiny_sizes() {
        let handle = crate::editor::test_handle();
        let root = std::env::temp_dir();
        let mut app = App::new(root.clone());
        app.enable_editor(root, handle);
        app.set_editor_layout(EditorLayout::Sidebar);
        for (width, height) in [(1, 1), (4, 2), (18, 4)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            assert_eq!(app.pane().area, Rect::new(0, 0, width, height));
        }
    }
}
