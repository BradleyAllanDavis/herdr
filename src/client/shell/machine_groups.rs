//! Per-machine grouping of the federated agent panel (`agent_panel_sort = "machines"`).
//!
//! Ordering lives here so the sidebar, keyboard navigation and the mobile list
//! agree on one sequence. Header rows are presentation only: they are never
//! part of the aggregate rows, so navigation and hit-testing cannot land on one.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::aggregate_navigation::AggregateAgentRow;
use super::*;

/// Whether the expanded agent panel should draw a header per machine.
///
/// A client with only its Local endpoint has nothing to separate, so it stays
/// identical to `priority`.
pub(super) fn wants_headers(endpoints: &[ClientShellEndpoint], config: &ClientShellConfig) -> bool {
    config.agent_panel_sort == crate::config::AgentPanelSortConfig::Machines && endpoints.len() > 1
}

/// Group rows by endpoint in catalog order (Local first), stale endpoints last,
/// with the `priority` key inside each group.
///
/// The sort is stable and `rows` arrives in endpoint order, so equal keys keep
/// the per-snapshot order from `ordered_agent_pane_ids`.
pub(super) fn sort_by_machine(
    endpoints: &[ClientShellEndpoint],
    rows: &mut [AggregateAgentRow<'_>],
) {
    let position = |row: &AggregateAgentRow<'_>| {
        endpoints
            .iter()
            .position(|endpoint| endpoint.endpoint_id == *row.endpoint.endpoint_id)
            .unwrap_or(usize::MAX)
    };
    rows.sort_by_cached_key(|row| {
        (
            row.endpoint.stale(),
            position(row),
            std::cmp::Reverse(status_priority(row.agent.agent_status)),
            std::cmp::Reverse(row.recency),
        )
    });
}

fn header_style(config: &ClientShellConfig, label: &str, stale: bool) -> Style {
    if stale {
        return Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD | Modifier::DIM);
    }
    let base = Style::default()
        .fg(config.palette.subtext0)
        .add_modifier(Modifier::BOLD);
    config.agents.machine_header.as_ref().map_or(base, |token| {
        crate::ui::apply_token_style(base, token.style_for_value(label))
    })
}

/// Accent for the tab bar (active tab and status pills) while `label` is the
/// active machine: the `fg` its `machine_header` rules give that label.
///
/// Without this every attached machine inherits the client's own accent, so a
/// remote's tabs and pills look like the local host's. `None` (no rule, no
/// `machine_header`, or a single endpoint) keeps the palette accent.
pub(super) fn active_machine_accent(
    endpoints: &[ClientShellEndpoint],
    config: &ClientShellConfig,
    label: &str,
) -> Option<ratatui::style::Color> {
    if endpoints.len() < 2 {
        return None;
    }
    let token = config.agents.machine_header.as_ref()?;
    Some(token.style_for_value(label).fg?.ratatui())
}

/// Draw `━━ label ━━━━━` across one row, styled by `[ui.sidebar.agents] machine_header`.
pub(super) fn render_machine_header(
    buffer: &mut Buffer,
    rect: Rect,
    label: &str,
    stale: bool,
    config: &ClientShellConfig,
) {
    use unicode_width::UnicodeWidthStr;

    const LEAD: &str = "━━ ";
    let width = rect.width as usize;
    // Keep room for the closing space and at least one trailing rule cell.
    let shown = crate::ui::truncate_end(label, width.saturating_sub(LEAD.width() + 2));
    let used = LEAD.width() + shown.width();
    let rule = "━".repeat(width.saturating_sub(used + 1));
    super::render::put_text(
        buffer,
        rect.x,
        rect.y,
        rect.width,
        &format!("{LEAD}{shown} {rule}"),
        // Rules match the machine's real label, so the color does not change
        // with the sidebar width.
        header_style(config, label, stale),
    );
}
