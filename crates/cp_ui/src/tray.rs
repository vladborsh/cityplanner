//! The tool tray (T-317): the panel of the open category above the bottom bar. A category's
//! sections are its tabs; the panel functions draw only the section of the active tab, so each
//! tab is one row of tiles. Outside a tray (a panel drawn on its own, as the tests do) every
//! section draws.

use bevy_egui::egui;

const ACTIVE: &str = "cp_tray_active";

/// The tab the tray is drawing, until [`leave`].
pub(crate) fn enter(ctx: &egui::Context, tab: &str) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(ACTIVE), tab.to_owned()));
}

pub(crate) fn leave(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(ACTIVE)));
}

/// Whether the tray is drawing (the sections lose their headers: the tab names them).
pub(crate) fn in_tray(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<String>(egui::Id::new(ACTIVE)))
        .is_some()
}

/// Whether the section titled `title` is the one to draw.
pub(crate) fn is_active(ctx: &egui::Context, title: &str) -> bool {
    ctx.data(|d| d.get_temp::<String>(egui::Id::new(ACTIVE)))
        .is_none_or(|a| a == title)
}
