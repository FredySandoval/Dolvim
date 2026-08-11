//! Compile-time visual themes.
//!
//! A theme describes semantic UI roles. Rendering chooses a role from application
//! state; rendered colors must never be inspected to recover that state.

use ratatui::style::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surface {
    pub background: Color,
    pub foreground: Color,
    pub secondary: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fill {
    pub background: Color,
    pub foreground: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryColors {
    pub folder: Color,
    pub file: Color,
    pub symlink: Color,
    pub executable: Color,
    pub cut: Color,
    pub unreachable: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub toolbar: Surface,
    pub panel: Surface,
    pub view: Surface,
    pub hover: Fill,
    pub selection: Fill,
    pub inactive_selection: Fill,
    pub active_cursor: Fill,
    pub inactive_cursor: Fill,
    pub accent: Color,
    pub separator: Color,
    pub error: Color,
    pub gauge_full: Color,
    pub gauge_empty: Color,
    pub entry: EntryColors,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

pub const BREEZE_LIGHT: Theme = Theme {
    name: "Breeze Light",
    toolbar: Surface {
        background: rgb(0xf4f5f6),
        foreground: rgb(0x232629),
        secondary: rgb(0x7f8c8d),
    },
    panel: Surface {
        background: rgb(0xeff0f1),
        foreground: rgb(0x232629),
        secondary: rgb(0x7f8c8d),
    },
    view: Surface {
        background: rgb(0xffffff),
        foreground: rgb(0x232629),
        secondary: rgb(0x7f8c8d),
    },
    hover: Fill {
        background: rgb(0xe0eff9),
        foreground: rgb(0x232629),
    },
    selection: Fill {
        background: rgb(0xc2e0f5),
        foreground: rgb(0x232629),
    },
    inactive_selection: Fill {
        background: rgb(0xd6dadd),
        foreground: rgb(0x232629),
    },
    active_cursor: Fill {
        background: rgb(0x3daee9),
        foreground: rgb(0xffffff),
    },
    inactive_cursor: Fill {
        background: rgb(0x7f8c8d),
        foreground: rgb(0xffffff),
    },
    accent: rgb(0x3daee9),
    separator: rgb(0xdcdcdc),
    error: rgb(0xda4453),
    gauge_full: rgb(0xc8cdd2),
    gauge_empty: rgb(0xc2e0f5),
    entry: EntryColors {
        folder: rgb(0x3daee9),
        file: rgb(0x63686d),
        symlink: rgb(0x1a8abe),
        executable: rgb(0x3a9c4a),
        cut: rgb(0xa0a5aa),
        unreachable: rgb(0xf67400),
    },
};

// Kept compiled so selecting a theme remains a one-line configuration change.
#[allow(dead_code)]
pub const BREEZE_DARK: Theme = Theme {
    name: "Breeze Dark",
    toolbar: Surface {
        background: rgb(0x31363b),
        foreground: rgb(0xeff0f1),
        secondary: rgb(0xaab0b5),
    },
    panel: Surface {
        background: rgb(0x232629),
        foreground: rgb(0xeff0f1),
        secondary: rgb(0xaab0b5),
    },
    view: Surface {
        background: rgb(0x1b1e20),
        foreground: rgb(0xeff0f1),
        secondary: rgb(0xaab0b5),
    },
    hover: Fill {
        background: rgb(0x2d404c),
        foreground: rgb(0xeff0f1),
    },
    selection: Fill {
        background: rgb(0x3daee9),
        foreground: rgb(0x1b1e20),
    },
    inactive_selection: Fill {
        background: rgb(0x4d5257),
        foreground: rgb(0xeff0f1),
    },
    active_cursor: Fill {
        background: rgb(0x3daee9),
        foreground: rgb(0x1b1e20),
    },
    inactive_cursor: Fill {
        background: rgb(0x7f8c8d),
        foreground: rgb(0x1b1e20),
    },
    accent: rgb(0x3daee9),
    separator: rgb(0x4d5257),
    error: rgb(0xed5b6a),
    gauge_full: rgb(0x4d5257),
    gauge_empty: rgb(0x3daee9),
    entry: EntryColors {
        folder: rgb(0x3daee9),
        file: rgb(0xbdc3c7),
        symlink: rgb(0x54a0cf),
        executable: rgb(0x27ae60),
        cut: rgb(0x696e73),
        unreachable: rgb(0xf67400),
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    const THEMES: &[Theme] = &[BREEZE_LIGHT, BREEZE_DARK];

    #[test]
    fn interactive_states_are_distinct() {
        for theme in THEMES {
            assert_ne!(theme.hover, theme.selection, "{}", theme.name);
            assert_ne!(
                theme.view.background, theme.selection.background,
                "{}",
                theme.name
            );
            assert_ne!(
                theme.selection.background, theme.selection.foreground,
                "{}",
                theme.name
            );
        }
    }

    #[test]
    fn surfaces_have_visible_text() {
        for theme in THEMES {
            for surface in [theme.toolbar, theme.panel, theme.view] {
                assert_ne!(surface.background, surface.foreground, "{}", theme.name);
                assert_ne!(surface.background, surface.secondary, "{}", theme.name);
            }
        }
    }
}
