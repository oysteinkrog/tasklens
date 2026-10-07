//! Canonical table (a) against the `Settings` and `UiState` defaults (E03-A2). The two TOML
//! blocks are copied verbatim from `docs/RECONCILIATION.md` section "(a) Settings schema". The
//! `[[minigraphs]]`, `[[detached_windows]]` and `[[tray_graphs]]` examples are commented out,
//! because those arrays are empty by default.

use tl_model::settings::{ColumnTables, Settings, UiState};

const SETTINGS_TOML: &str = r#"schema_version = 1

[general]
start_tab = "processes"        # processes | performance | services | startup | connections | information | last_used
language = "system"            # system | en-US | de-DE | es-ES | fr-FR | ja-JP | ko-KR | pt-BR | ru-RU | uk-UA | zh-CN | nb-NO

[theme]
active = "Default Dark"        # theme name
follow_system = false
light = "Default Light"
dark = "Default Dark"

[appearance]
ui_scale_pct = 100             # 75..=200, step 5
row_height_pct = 100           # 80..=150, step 5
decorations = "client"         # client | server  (label "Use system title bar"; restart)
rounded_corners = true
heat_coloring = true
heat_intensity_pct = 50        # 0..=100
shadow_intensity_pct = 100     # 0..=100

[motion]
animations = true              # tab, theme, toast motion; ANDed with system reduce-motion
smooth_graphs = true           # chart sliding
smooth_scroll = true
smooth_scroll_amount = 15      # 1..=50
tab_switch_ms = 250            # 0..=500, step 10

[units]
network = "bits"               # bits | bytes
network_auto_scale = false

[processes]
interval_ms = 1000             # 250..=10000; menu 250 500 1000 2000 5000 10000
mode = "grouped"               # grouped | tree | flat
tree_guide_lines = false
highlight_new_exited = true
filter_include_children = false
hide_system = false
subtree_totals = false         # "Show subtree totals on parent processes"
dump_type = "mini"             # mini | full

[tables]                        # every table (R-075)
column_resize = "proportional" # proportional | last_column
column_separators = false
dim_zero_values = false

[properties]
hide_unnamed_handles = true
shared_objects_only = true
env_mask = true

[performance]
graph_range_s = 60             # 30 | 60 | 300 | 600 (300 and 600 from P2)
cpu_graph_mode = "per_core"    # overall | per_core | all_cores_overlaid | all_cores_stacked
cpu_kernel = true
cpu_secondary_graph = "power"  # none | power | clock | temperature
gpu_optional_graphs = []       # subset of ["power", "clocks"]
hidden_gpu_engines = []        # engine keys
memory_composition = true
memory_swap_graph = true
show_disks = true
show_gpus = true
show_fans = true
show_batteries = true
show_network_wired = true
show_network_wireless = true
show_network_vpn = true        # VPN and tunnel
show_network_virtual = false
show_network_other = true
show_disconnected_fans = false
device_order = []              # device keys (PERF-005)
device_overrides = {}          # device key -> "show" | "hide"
curved_lines = false
network_dynamic_scale = true   # false = link speed

[fans]                          # per fan key, P3
# "<key>" = { label = "...", temp = "<hwmon name>:<path>:tempN" }
# both fields optional: label omitted when empty, temp omitted for None; an entry with neither is removed (R-221)

[graphs]
minigraph_cpu = true
minigraph_memory = true
minigraph_disk = true
minigraph_network = true
minigraph_gpu = true
minigraph_vram = true
minigraph_gpu_temperature = false
keep_detached_on_close = false

[history]                       # P3; D-003
enabled = false
retention_h = 6                # 1..=12; menu 1 2 3 4 6 8 12
persist = true                 # false = delete segments at shutdown
max_disk_mb = 256              # not in menus

[window]
close_with_esc = false
always_on_top = false

[tray]                          # P3
close_to_tray = false
minimize_to_tray = false
always_show = false

[startup]
start_at_login = false
start_minimized = false

[network]
auto_fetch_public_ip = false
public_ip_notice_accepted = false

[privilege]                     # P3/P4
unlock_at_launch = false
passwordless = false

[rendering]                     # P4
gpu = "auto"                   # auto | software | pci:<bus id> | name:<adapter name> (R-237)

[services]
unit_types = ["service", "socket", "timer", "mount"]
filter_states = []             # subset of running failed stopped disabled

[lowmem]                        # P3
enabled = false

[debug]
show_fps = false
"#;

const STATE_TOML: &str = r#"schema_version = 1

[window]
width = 1280                   # logical px; clamped to >= 800
height = 800                   # clamped to >= 500
maximized = false

[tabs]
last_tab = "processes"

[processes]
history_mode = false
group_expanded = { apps = true, background = true, system = true }
virustotal_notice_accepted = false   # R-095

[services]
group_expanded = { system = true, user = true }

[performance]
nav_width = 252                # 180..=400
selected_device = "cpu"

# one table per view: processes, history, services, startup, connections
[lowmem]                       # P3 (R-152)
oom_score_adjust = -900        # the OOMScoreAdjust the installed watcher unit accepted; 0 when the manager refused -900 (U-24)

[sort.processes]
column = "pid"
ascending = true
[sort.history]
column = "cpu"
ascending = false
[sort.services]
column = "name"
ascending = true
[sort.startup]
column = "status"
ascending = true
[sort.connections]
column = "proto"
ascending = true

[columns.processes]            # same shape for the other four tables
order = ["name", "user", "pid", "cpu", "memory", "disk", "network", "gpu", "description"]
visible = ["name", "user", "pid", "cpu", "memory", "disk", "network", "gpu", "description"]
widths = { name = 248, user = 92, pid = 96, cpu = 63, memory = 92, disk = 82, network = 92, gpu = 72, description = 342, power = 101 }

# [[minigraphs]]                 # P1 hide and range, P3 colour and detach
# source = "cpu"                 # cpu | memory | disk | network | gpu | vram | gpu_temp | gpu:<device key> | vram:<device key> | gpu_temp:<device key> | process:<boot_id 16 hex>:<pid>:<start_ticks> | name:<image name> | app:<app id>
#                                # bare gpu, vram, gpu_temp = first GPU in sidebar order (R-085); boot ID per R-100
# column = "cpu"                 # for process sources
# range_s = 60
# color = "accent"               # accent | <one of the 17 names, lower case>
# slot = 0
# detached = false
# window = 0                     # detached window id; ignored when detached = false (R-084)

# [[detached_windows]]           # P3 (R-084)
# id = 0
# width = 180                    # logical px
# height = 90
# x = 0                          # x and y written and applied on KDE only, through the KWin bridge
# y = 0

# [[tray_graphs]]                # P3
# source = "cpu"
# column = "cpu"
# color = "accent"
# range_s = 60
"#;

#[test]
fn settings_toml_equals_default() {
    let parsed: Settings = toml::from_str(SETTINGS_TOML).unwrap();
    assert_eq!(parsed, Settings::default());
}

#[test]
fn state_toml_equals_default() {
    let mut parsed: UiState = toml::from_str(STATE_TOML).unwrap();
    // Table (a) shows only `[columns.processes]`; the other four tables keep their defaults.
    let defaults = ColumnTables::default();
    parsed.columns.history = defaults.history;
    parsed.columns.services = defaults.services;
    parsed.columns.startup = defaults.startup;
    parsed.columns.connections = defaults.connections;
    // Table (a) shows an installed unit; the default has none.
    assert_eq!(parsed.lowmem.oom_score_adjust, Some(-900));
    parsed.lowmem = Default::default();
    assert_eq!(parsed, UiState::default());
}

#[test]
fn empty_files_are_defaults() {
    assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
    assert_eq!(toml::from_str::<UiState>("").unwrap(), UiState::default());
}

#[test]
fn unknown_keys_ignored() {
    let text = "schema_version = 1\n\n[general]\nfoo = 1\n\n[bogus]\nbar = \"baz\"\n";
    assert_eq!(
        toml::from_str::<Settings>(text).unwrap(),
        Settings::default()
    );
    assert_eq!(toml::from_str::<UiState>(text).unwrap(), UiState::default());
}

#[test]
fn settings_save_load_roundtrip() {
    let text = toml::to_string(&Settings::default()).unwrap();
    assert_eq!(
        toml::from_str::<Settings>(&text).unwrap(),
        Settings::default()
    );
}

#[test]
fn state_save_load_roundtrip() {
    let text = toml::to_string(&UiState::default()).unwrap();
    assert_eq!(
        toml::from_str::<UiState>(&text).unwrap(),
        UiState::default()
    );
}

#[test]
fn lowmem_state_key_left_out_when_none_and_clamped() {
    let text = toml::to_string(&UiState::default()).unwrap();
    assert!(!text.contains("oom_score_adjust"), "{text}");
    let mut state = UiState::default();
    state.lowmem.oom_score_adjust = Some(0);
    let text = toml::to_string(&state).unwrap();
    assert!(text.contains("[lowmem]\noom_score_adjust = 0\n"), "{text}");
    assert_eq!(toml::from_str::<UiState>(&text).unwrap(), state);
    state.lowmem.oom_score_adjust = Some(-5000);
    assert_eq!(state.clamp(), vec!["lowmem.oom_score_adjust"]);
    assert_eq!(state.lowmem.oom_score_adjust, Some(-1000));
}
