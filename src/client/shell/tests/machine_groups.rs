//! `agent_panel_sort = "machines"`: per-machine groups with header rows.

use super::*;
use crate::api::schema::AgentStatus;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};
use crate::config::{AgentPanelSortConfig, AgentSidebarToken};
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

fn profile(id_digit: char, label: &str) -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: ProfileId::parse(id_digit.to_string().repeat(32)).unwrap(),
        label: label.into(),
        target: format!("dev@{label}.example"),
        session: "agents".into(),
        enabled: true,
    }
}

fn agent(pane: &str, name: &str, status: AgentStatus, seq: u64) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: seq,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

fn machines_config(configure: impl FnOnce(&mut Config)) -> ClientShellConfig {
    let mut config = Config::default();
    config.ui.agent_panel_sort = AgentPanelSortConfig::Machines;
    config.ui.sidebar.agents.rows = vec![vec![AgentSidebarToken::Agent]];
    configure(&mut config);
    ClientShellConfig::from_config(&config)
}

/// Local plus the given saved machines, each with the given agents.
/// `online` false leaves a machine reconnecting (stale) with its cached snapshot.
fn federated(
    config: ClientShellConfig,
    machines: &[(&str, char, bool, Vec<ClientShellAgent>)],
    local_agents: Vec<ClientShellAgent>,
) -> (ClientShellState, Vec<ClientEndpointId>) {
    let mut state = ClientShellState::new(config);
    let profiles = machines
        .iter()
        .map(|(label, digit, _, _)| profile(*digit, label))
        .collect::<Vec<_>>();
    state.set_endpoint_catalog(&profiles);
    let mut local = snapshot();
    local.agents = local_agents;
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    let mut ids = Vec::new();
    for ((_, _, online, agents), profile) in machines.iter().zip(&profiles) {
        let id = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_status(&id, ClientEndpointStatus::Online);
        let mut remote = snapshot();
        remote.boot_id = format!("boot-{}", profile.label);
        remote.agents = agents.clone();
        state.set_endpoint_snapshot(&id, Box::new(remote));
        if !online {
            state.set_endpoint_status(&id, ClientEndpointStatus::Reconnecting);
        }
        ids.push(id);
    }
    (state, ids)
}

fn frame_lines(state: &mut ClientShellState) -> Vec<String> {
    let frame = state.compose(100, 28).expect("frame");
    frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect()
}

fn line_index(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` not in frame:\n{}", lines.join("\n")))
}

fn order(state: &ClientShellState) -> Vec<(String, String)> {
    super::super::aggregate_navigation::aggregate_agent_rows(
        &state.endpoints,
        state.config.agent_panel_sort,
    )
    .into_iter()
    .map(|row| {
        (
            row.endpoint.label.to_owned(),
            row.agent.name.clone().unwrap_or_default(),
        )
    })
    .collect()
}

#[test]
fn groups_by_endpoint_order_with_priority_inside_and_stale_last() {
    let (state, _) = federated(
        machines_config(|_| {}),
        &[
            // The stale machine sits before "tank" in catalog order but sorts last.
            (
                "mini",
                '1',
                false,
                vec![agent("p1", "mini blocked", AgentStatus::Blocked, 9)],
            ),
            (
                "tank",
                '2',
                true,
                vec![
                    agent("p1", "tank idle", AgentStatus::Idle, 1),
                    agent("p2", "tank blocked", AgentStatus::Blocked, 2),
                    agent("p3", "tank working", AgentStatus::Working, 3),
                ],
            ),
        ],
        vec![agent("p1", "local idle", AgentStatus::Idle, 1)],
    );

    let names = order(&state)
        .into_iter()
        .map(|(machine, name)| format!("{machine}/{name}"))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "Local/local idle",
            "tank/tank blocked",
            "tank/tank working",
            "tank/tank idle",
            "mini/mini blocked",
        ]
    );
}

#[test]
fn priority_sort_still_interleaves_machines() {
    let (mut state, _) = federated(
        machines_config(|config| {
            config.ui.agent_panel_sort = AgentPanelSortConfig::Priority;
        }),
        &[(
            "tank",
            '2',
            true,
            vec![agent("p1", "tank blocked", AgentStatus::Blocked, 2)],
        )],
        vec![agent("p1", "local idle", AgentStatus::Idle, 1)],
    );
    assert_eq!(order(&state)[0].1, "tank blocked");
    let lines = frame_lines(&mut state);
    assert!(lines.iter().all(|line| !line.contains("━━")));
}

#[test]
fn renders_one_header_per_non_empty_machine() {
    let (mut state, _) = federated(
        machines_config(|_| {}),
        &[
            (
                "tank",
                '2',
                true,
                vec![agent("p1", "tank agent", AgentStatus::Working, 1)],
            ),
            ("neo", '3', true, Vec::new()),
        ],
        vec![agent("p1", "local agent", AgentStatus::Idle, 1)],
    );
    let lines = frame_lines(&mut state);
    let local = line_index(&lines, "━━ Local ");
    let local_agent = line_index(&lines, "local agent");
    let tank = line_index(&lines, "━━ tank ");
    let tank_agent = line_index(&lines, "tank agent");
    assert_eq!([local + 1, tank + 1], [local_agent, tank_agent]);
    assert_eq!(tank, local_agent + 1);
    assert!(lines.iter().all(|line| !line.contains("━━ neo")));
    // The panel title row sits two rows above the first group header.
    assert!(
        lines[local - 2].contains("machines"),
        "{}",
        lines[local - 2]
    );
}

#[test]
fn single_machine_has_no_header() {
    let mut state = ClientShellState::new(machines_config(|_| {}));
    let mut local = snapshot();
    local.agents = vec![
        agent("p1", "idle one", AgentStatus::Idle, 1),
        agent("p2", "blocked one", AgentStatus::Blocked, 2),
    ];
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    let lines = frame_lines(&mut state);
    assert!(lines.iter().all(|line| !line.contains("━━")));
    // Same ordering as `priority`.
    assert!(line_index(&lines, "blocked one") < line_index(&lines, "idle one"));
}

#[test]
fn header_uses_configured_rules_and_dims_stale_machines() {
    let config = machines_config(|config| {
        config.ui.sidebar.agents.machine_header = Some(
            toml::from_str::<crate::config::SidebarConfig>(
                r##"[agents]
machine_header = { token = "machine", rules = [{ equals = "tank", fg = "#2aa198" }, { equals = "mini", fg = "#dc322f" }] }"##,
            )
            .unwrap()
            .agents
            .machine_header
            .unwrap(),
        );
    });
    let (mut state, _) = federated(
        config,
        &[
            (
                "tank",
                '2',
                true,
                vec![agent("p1", "tank agent", AgentStatus::Idle, 1)],
            ),
            (
                "mini",
                '3',
                false,
                vec![agent("p1", "mini agent", AgentStatus::Idle, 1)],
            ),
        ],
        vec![agent("p1", "local agent", AgentStatus::Idle, 1)],
    );
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let width = frame.width as usize;
    let lines = frame_lines(&mut state);
    let (subtext0, overlay0) = (state.config.palette.subtext0, state.config.palette.overlay0);
    let cell_for = |needle: &str| {
        let row = line_index(&lines, needle);
        let column = lines[row].chars().position(|c| c == '━').unwrap();
        buffer.content()[row * width + column].clone()
    };

    let tank = cell_for("━━ tank ");
    assert_eq!(tank.fg, ratatui::style::Color::Rgb(0x2a, 0xa1, 0x98));
    assert!(tank.modifier.contains(ratatui::style::Modifier::BOLD));
    // No rule matches Local: plain bold label in the palette's own color.
    let local = cell_for("━━ Local ");
    assert_eq!(local.fg, subtext0);
    // The stale machine ignores its rule and uses the dimmed stale treatment.
    let mini = cell_for("━━ mini ");
    assert_eq!(mini.fg, overlay0);
    assert!(mini.modifier.contains(ratatui::style::Modifier::DIM));
}

#[test]
fn navigation_skips_headers_and_follows_group_order() {
    let (state, ids) = federated(
        machines_config(|_| {}),
        &[
            (
                "tank",
                '2',
                true,
                vec![agent("p1", "tank agent", AgentStatus::Blocked, 1)],
            ),
            (
                "mini",
                '3',
                false,
                vec![agent("p1", "mini agent", AgentStatus::Blocked, 1)],
            ),
        ],
        vec![agent("p1", "local agent", AgentStatus::Idle, 1)],
    );
    let targets = super::super::aggregate_navigation::online_agent_targets(
        &state.endpoints,
        AgentPanelSortConfig::Machines,
    );
    // Only online agents, in display order, with no entry for header rows.
    assert_eq!(
        targets
            .iter()
            .map(|target| target.endpoint_id.clone())
            .collect::<Vec<_>>(),
        vec![ClientEndpointId::Local, ids[0].clone()]
    );
}

#[test]
fn clicks_land_on_agents_below_headers_and_ignore_headers() {
    let (mut state, ids) = federated(
        machines_config(|_| {}),
        &[(
            "tank",
            '2',
            true,
            vec![agent("p7", "tank agent", AgentStatus::Working, 1)],
        )],
        vec![agent("p1", "local agent", AgentStatus::Idle, 1)],
    );
    let lines = frame_lines(&mut state);
    let header_row = line_index(&lines, "━━ tank ") as u16;
    let agent_row = line_index(&lines, "tank agent") as u16;
    assert_eq!(agent_row, header_row + 1);
    let click = |state: &mut ClientShellState, row: u16, column: u16| {
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        })])
    };
    let column = state.hits.agent_body.x + 2;

    let outcome = click(&mut state, header_row, column);
    assert!(
        outcome.actions.is_empty(),
        "header click: {:?}",
        outcome.actions.len()
    );

    let outcome = click(&mut state, agent_row, column);
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint {
            endpoint_id,
            target: Some(ClientEndpointFocusTarget::Pane(pane_id)),
        }] if endpoint_id == &ids[0] && pane_id == "p7"
    ));
}

#[test]
fn scroll_metrics_count_header_rows() {
    let crowded = (0..40)
        .map(|index| {
            agent(
                &format!("p{index}"),
                &format!("agent {index}"),
                AgentStatus::Idle,
                index,
            )
        })
        .collect::<Vec<_>>();
    let (mut state, _) = federated(
        machines_config(|_| {}),
        &[("tank", '2', true, crowded.clone())],
        crowded,
    );
    state.compose(100, 28).expect("frame");
    let metrics = state.hits.agent_scroll_metrics.expect("list overflows");
    // 80 agent lines plus one header per machine.
    assert_eq!(
        metrics.max_offset_from_bottom,
        80 + 2 - usize::from(state.hits.agent_body.height)
    );
}

#[test]
fn header_fits_narrow_widths_and_truncates_long_labels() {
    let config = machines_config(|_| {});
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 12, 1));
    let area = buffer.area;
    super::super::machine_groups::render_machine_header(
        &mut buffer,
        area,
        "a-very-long-machine-name",
        false,
        &config,
    );
    let text = (0..12)
        .map(|x| buffer[(x, 0)].symbol().to_owned())
        .collect::<String>();
    assert!(text.starts_with("━━ a-very…"), "{text}");
    assert!(text.contains('…'), "{text}");
    assert!(text.ends_with(" ━"), "{text}");

    let mut tiny = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 2, 1));
    let area = tiny.area;
    super::super::machine_groups::render_machine_header(&mut tiny, area, "x", false, &config);
}

#[test]
fn local_endpoint_reads_as_host_local() {
    use super::super::endpoints::local_endpoint_label;
    assert_eq!(local_endpoint_label(Some("morpheus")), "morpheus (local)");
    // Short name only: macOS reports `host.local` or a FQDN.
    assert_eq!(
        local_endpoint_label(Some("dozer.lan.example")),
        "dozer (local)"
    );
    assert_eq!(local_endpoint_label(Some("")), "Local");
    assert_eq!(local_endpoint_label(Some(".local")), "Local");
    assert_eq!(local_endpoint_label(None), "Local");

    // The label reaches the machines list and the agent panel header, and a
    // `starts_with` rule on the host name colors it like the remote view would.
    let config = machines_config(|config| {
        config.ui.sidebar.agents.machine_header = Some(
            toml::from_str::<crate::config::SidebarConfig>(
                r##"[agents]
machine_header = { token = "machine", rules = [{ starts_with = "morpheus", fg = "#859900" }] }"##,
            )
            .unwrap()
            .agents
            .machine_header
            .unwrap(),
        );
    })
    .with_local_hostname(Some("morpheus.local"));
    let (mut state, _) = federated(
        config,
        &[(
            "tank",
            '2',
            true,
            vec![agent("p1", "tank agent", AgentStatus::Idle, 1)],
        )],
        vec![agent("p1", "local agent", AgentStatus::Idle, 1)],
    );
    let frame = state.compose(100, 28).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let width = frame.width as usize;
    let lines = frame_lines(&mut state);
    assert!(
        lines.iter().any(|line| line.contains("morpheus (local)")),
        "machines list: {}",
        lines.join("\n")
    );
    assert!(!lines.iter().any(|line| line.contains("Local")));
    let row = line_index(&lines, "━━ morpheus (local) ");
    let column = lines[row].chars().position(|c| c == '━').unwrap();
    assert_eq!(
        buffer.content()[row * width + column].fg,
        ratatui::style::Color::Rgb(0x85, 0x99, 0x00)
    );
}

#[test]
fn header_rule_matches_the_full_label_even_when_truncated() {
    let mut config = Config::default();
    config.ui.sidebar.agents.machine_header = Some(
        toml::from_str::<crate::config::SidebarConfig>(
            r##"[agents]
machine_header = { token = "machine", rules = [{ equals = "a-very-long-machine-name", fg = "#dc322f" }] }"##,
        )
        .unwrap()
        .agents
        .machine_header
        .unwrap(),
    );
    let config = ClientShellConfig::from_config(&config);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 12, 1));
    let area = buffer.area;
    super::super::machine_groups::render_machine_header(
        &mut buffer,
        area,
        "a-very-long-machine-name",
        false,
        &config,
    );
    assert_eq!(
        buffer[(4, 0)].fg,
        ratatui::style::Color::Rgb(0xdc, 0x32, 0x2f)
    );
}

#[test]
fn sort_toggle_reaches_machines_and_the_choice_survives_a_restart() {
    let path = std::env::temp_dir().join(format!(
        "herdr-shell-machines-sort-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let sort = |state: &ClientShellState| state.config.agent_panel_sort;
    let click_toggle = |state: &mut ClientShellState| {
        state.compose(100, 28).expect("frame");
        let toggle = state.hits.agent_sort_toggle;
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: toggle.x,
            row: toggle.y,
            modifiers: KeyModifiers::empty(),
        })]);
    };
    let (mut state, _) = federated(
        machines_config(|_| {}).with_preferences_path(path.clone()),
        &[("tank", '2', true, Vec::new())],
        Vec::new(),
    );

    // machines -> spaces -> priority -> machines: a saved click overrides the
    // config file, so the cycle has to come back to machines.
    assert_eq!(sort(&state), AgentPanelSortConfig::Machines);
    click_toggle(&mut state);
    assert_eq!(sort(&state), AgentPanelSortConfig::Spaces);
    click_toggle(&mut state);
    assert_eq!(sort(&state), AgentPanelSortConfig::Priority);
    click_toggle(&mut state);
    assert_eq!(sort(&state), AgentPanelSortConfig::Machines);

    let reloaded =
        ClientShellState::new(machines_config(|_| {}).with_preferences_path(path.clone()));
    assert_eq!(sort(&reloaded), AgentPanelSortConfig::Machines);
    assert!(reloaded.agent_panel_sort_manual);
    std::fs::remove_file(path).expect("remove sort preferences");

    // With only the Local endpoint, machines separates nothing: two-state.
    let mut local_only = ClientShellState::new(machines_config(|config| {
        config.ui.agent_panel_sort = AgentPanelSortConfig::Priority;
    }));
    local_only.set_snapshot(Box::new(snapshot()));
    local_only.set_pane_surface(surface());
    click_toggle(&mut local_only);
    assert_eq!(sort(&local_only), AgentPanelSortConfig::Spaces);
}

#[test]
fn tab_bar_accent_follows_the_active_machines_header_color() {
    let config = machines_config(|config| {
        config.ui.sidebar.agents.machine_header = Some(
            toml::from_str::<crate::config::SidebarConfig>(
                r##"[agents]
machine_header = { token = "machine", rules = [{ equals = "tank", fg = "#2aa198" }] }"##,
            )
            .unwrap()
            .agents
            .machine_header
            .unwrap(),
        );
    });
    let palette_accent = config.palette.accent;
    let tank_color = ratatui::style::Color::Rgb(0x2a, 0xa1, 0x98);
    let (mut state, ids) = federated(
        config,
        &[("tank", '2', true, Vec::new())],
        vec![agent("p1", "local agent", AgentStatus::Idle, 1)],
    );
    let cells_with_bg = |state: &mut ClientShellState, color| {
        let frame = state.compose(100, 28).expect("frame");
        let buffer = frame.to_ratatui_buffer().expect("buffer");
        buffer
            .content()
            .iter()
            .filter(|cell| cell.bg == color)
            .count()
    };

    // Local has no rule, so its tabs keep the client palette accent.
    assert!(cells_with_bg(&mut state, palette_accent) > 0);
    assert_eq!(cells_with_bg(&mut state, tank_color), 0);

    // Attached to tank, the focused tab takes tank's header color instead.
    assert!(state.activate_endpoint_projection(&ids[0]));
    let mut tank_surface = surface();
    tank_surface.boot_id = "boot-tank".into();
    state.set_pane_surface(tank_surface);
    assert!(cells_with_bg(&mut state, tank_color) > 0);
    assert_eq!(cells_with_bg(&mut state, palette_accent), 0);
}
