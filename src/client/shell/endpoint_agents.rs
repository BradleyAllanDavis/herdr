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

impl ClientShellState {
    pub(super) fn reveal_endpoint_agent(
        &mut self,
        endpoint_id: &ClientEndpointId,
        pane_id: &str,
        body_height: u16,
    ) {
        if body_height == 0 {
            return;
        }
        // Same entries the panel renders, so machine headers count toward the
        // scroll metrics and the target index matches what is on screen.
        let entries = list_entries(
            agent_rows(&self.endpoints, &self.active_endpoint_id, &self.config),
            super::machine_groups::wants_headers(&self.endpoints, &self.config),
        );
        let Some(target) = entries.iter().position(|entry| {
            matches!(
                entry,
                ListEntry::Agent(row)
                    if &row.endpoint_id == endpoint_id && row.agent.pane_id == pane_id
            )
        }) else {
            return;
        };
        let heights = entries
            .iter()
            .map(|entry| match entry {
                ListEntry::Header { .. } => 1,
                ListEntry::Agent(row) => row.agent.rows.len().max(1).min(u16::MAX as usize) as u16,
            })
            .collect::<Vec<_>>();
        let mut gaps = vec![self.config.agents.row_gap; entries.len()];
        if let Some(last) = gaps.last_mut() {
            *last = 0;
        }
        self.agent_scroll = super::scroll::list_scroll_start_to_reveal(
            &heights,
            &gaps,
            body_height,
            self.agent_scroll,
            target,
        );
    }
}

/// One entry of the expanded agent list: a machine header or an agent.
enum ListEntry {
    Header { label: String, stale: bool },
    Agent(EndpointAgentRow),
}

/// Wrap rows as list entries, opening a header whenever the endpoint changes.
/// Headers are only drawn when every endpoint's rows form one contiguous run
/// (true for the `machines` sort); an agent view with its own sort can
/// interleave machines, and a header per run would then repeat a machine.
fn list_entries(rows: Vec<EndpointAgentRow>, headers: bool) -> Vec<ListEntry> {
    let headers = headers && endpoints_are_contiguous(rows.iter().map(|row| &row.endpoint_id));
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

fn endpoints_are_contiguous<'a>(ids: impl Iterator<Item = &'a ClientEndpointId>) -> bool {
    let mut seen: Vec<&ClientEndpointId> = Vec::new();
    for id in ids {
        match seen.last() {
            Some(last) if *last == id => {}
            _ if seen.contains(&id) => return false,
            _ => seen.push(id),
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh(id: &str) -> ClientEndpointId {
        ClientEndpointId::Ssh(crate::client::endpoint::ProfileId::parse(id).unwrap())
    }

    #[test]
    fn endpoints_are_contiguous_accepts_one_run_per_endpoint() {
        let a = ssh("0123456789abcdef0123456789abcdef");
        let ids = [
            ClientEndpointId::Local,
            ClientEndpointId::Local,
            a.clone(),
            a,
        ];
        assert!(endpoints_are_contiguous(ids.iter()));
        assert!(endpoints_are_contiguous([].iter()));
    }

    #[test]
    fn endpoints_are_contiguous_rejects_an_interleaved_endpoint() {
        // An agent view with its own sort can put Local, remote, Local: a
        // header per run would then name Local twice.
        let ids = [
            ClientEndpointId::Local,
            ssh("0123456789abcdef0123456789abcdef"),
            ClientEndpointId::Local,
        ];
        assert!(!endpoints_are_contiguous(ids.iter()));
    }
}
