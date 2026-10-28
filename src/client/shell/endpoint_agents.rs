use super::render::put_text;
use super::*;

pub(super) fn render_collapsed(
    buffer: &mut Buffer,
    area: Rect,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let rows = agent_rows(endpoints, active_endpoint_id, config);
    for (index, row) in rows.into_iter().take(area.height as usize).enumerate() {
        let rect = Rect::new(area.x, area.y + index as u16, area.width, 1);
        if row.agent.focused {
            buffer.set_style(rect, Style::default().bg(config.palette.active_row_bg));
        }
        let initial = row.machine_label.chars().next().unwrap_or('?');
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width,
            &format!(
                "{initial}{}",
                status_icon(row.agent.status, config.status_indicators)
            ),
            Style::default()
                .fg(if row.stale {
                    config.palette.overlay0
                } else {
                    status_color(row.agent.status, &config.palette)
                })
                .add_modifier(if row.stale {
                    Modifier::DIM
                } else {
                    Modifier::empty()
                }),
        );
        hits.endpoint_agents
            .push((rect, row.endpoint_id, row.agent.pane_id));
    }
}

pub(super) fn render_expanded(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if !super::agent_sidebar::render_agent_panel_header(
        buffer,
        area,
        agent_view_label,
        config,
        hits,
    ) {
        return;
    }
    let rows = agent_rows(endpoints, active_endpoint_id, config);
    let entries = list_entries(
        rows,
        super::machine_groups::wants_headers(endpoints, config),
    );
    super::agent_sidebar::render_agent_list(
        buffer,
        area,
        &entries,
        agent_view_label.map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |entry| match entry {
            ListEntry::Header { .. } => 1,
            ListEntry::Agent(row) => row.agent.rows.len(),
        },
        |buffer, rect, entry, hits| {
            let row = match entry {
                // Headers register no hit area, so clicks on them fall through.
                ListEntry::Header { label, stale } => {
                    super::machine_groups::render_machine_header(
                        buffer, rect, label, *stale, config,
                    );
                    return;
                }
                ListEntry::Agent(row) => row,
            };
            super::agent_sidebar::render_agent_row(buffer, rect, &row.agent, config);
            if row.stale {
                buffer.set_style(
                    rect,
                    Style::default()
                        .fg(config.palette.overlay0)
                        .add_modifier(Modifier::DIM),
                );
            }
            hits.endpoint_agents
                .push((rect, row.endpoint_id.clone(), row.agent.pane_id.clone()));
        },
    );
}

/// One entry of the expanded agent list: a machine header or an agent.
enum ListEntry {
    Header { label: String, stale: bool },
    Agent(EndpointAgentRow),
}

/// Wrap rows as list entries, opening a header whenever the endpoint changes.
/// Rows of one endpoint are contiguous in every sort that asks for headers.
fn list_entries(rows: Vec<EndpointAgentRow>, headers: bool) -> Vec<ListEntry> {
    let mut entries = Vec::with_capacity(rows.len());
    let mut previous: Option<ClientEndpointId> = None;
    for row in rows {
        if headers && previous.as_ref() != Some(&row.endpoint_id) {
            previous = Some(row.endpoint_id.clone());
            entries.push(ListEntry::Header {
                label: row.machine_label.clone(),
                stale: row.stale,
            });
        }
        entries.push(ListEntry::Agent(row));
    }
    entries
}

struct EndpointAgentRow {
    endpoint_id: ClientEndpointId,
    machine_label: String,
    stale: bool,
    agent: super::agent_sidebar::AgentRow,
}

fn agent_rows(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
) -> Vec<EndpointAgentRow> {
    let mut rendered_rows = endpoints
        .iter()
        .filter_map(|endpoint| {
            endpoint.snapshot.as_deref().map(|snapshot| {
                super::agent_sidebar::agent_rows(snapshot, config, Some(&endpoint.label))
                    .into_iter()
                    .map(|agent| ((endpoint.endpoint_id.clone(), agent.pane_id.clone()), agent))
                    .collect::<Vec<_>>()
            })
        })
        .flatten()
        .collect::<HashMap<_, _>>();

    super::aggregate_navigation::aggregate_agent_rows(endpoints, config.agent_panel_sort)
        .into_iter()
        .filter_map(|row| {
            let key = (row.endpoint.endpoint_id.clone(), row.agent.pane_id.clone());
            let mut agent = rendered_rows.remove(&key)?;
            agent.focused &= row.endpoint.endpoint_id == active_endpoint_id;
            Some(EndpointAgentRow {
                endpoint_id: row.endpoint.endpoint_id.clone(),
                machine_label: row.endpoint.label.to_owned(),
                stale: row.endpoint.stale(),
                agent,
            })
        })
        .collect()
}
