//! Shared sizing constants.
//!
//! Only values that appear in several places live here — a constant with one
//! call site is just a rename. Everything else stays inline where it is used.

use egui::{CornerRadius, Margin};

/// Icon point sizes. Three steps instead of the five that had accumulated.
pub(crate) mod icon {
    pub(crate) const SM: f32 = 12.0;
    pub(crate) const MD: f32 = 14.0;
    pub(crate) const LG: f32 = 15.0;
}

/// Fixed chrome and control sizes, in points.
pub(crate) mod size {
    pub(crate) const TOP_BAR: f32 = 48.0;
    pub(crate) const STATUS_BAR: f32 = 30.0;
    pub(crate) const ACTIVITY_RAIL: f32 = 64.0;
    pub(crate) const RAIL_BUTTON: [f32; 2] = [52.0, 48.0];
    pub(crate) const SIDEBAR_DEFAULT: f32 = 252.0;
    pub(crate) const SIDEBAR_MIN: f32 = 220.0;
    pub(crate) const SIDEBAR_MAX: f32 = 320.0;
    /// Height of a selectable row in the sidebars.
    pub(crate) const ROW: f32 = 34.0;
    /// Height of a single-line input or primary control.
    pub(crate) const FIELD: f32 = 36.0;
    /// Square icon-only button.
    pub(crate) const ICON_BUTTON: f32 = 28.0;
}

pub(crate) mod radius {
    use super::CornerRadius;

    pub(crate) const SM: CornerRadius = CornerRadius::same(4);
    pub(crate) const MD: CornerRadius = CornerRadius::same(6);
}

pub(crate) mod pad {
    use super::Margin;

    /// Workspace surfaces (request, scenario, mock, history, environment).
    pub(crate) const WORKSPACE: Margin = Margin {
        left: 18,
        right: 18,
        top: 14,
        bottom: 14,
    };
    /// Sidebar and top-bar chrome.
    pub(crate) const CHROME: Margin = Margin {
        left: 12,
        right: 12,
        top: 10,
        bottom: 10,
    };
    /// The request composer, which sits between the two.
    pub(crate) const COMPOSER: Margin = Margin {
        left: 16,
        right: 16,
        top: 10,
        bottom: 10,
    };
}
