# English (en-US) catalogue. Embedded in tl-ui with include_str!.
# Keys are <area>-<entry>-<slug> in kebab case (R-132). Other epics append their own keys.
# Numbers never pass through Fluent (R-081): callers format them and pass the text in.

## Foundation (E15, E16)

fnd-validity-warming-up = Collecting first sample
fnd-validity-stale = Last updated { $age } ago
fnd-validity-restricted = Needs administrator access.
fnd-validity-restricted-p3 = Needs administrator access. Click the shield to grant it.
fnd-validity-unsupported = Not available on this device
fnd-validity-missing = No longer present
fnd-validity-failed = Could not read: { $error }
fnd-menu-no-items = No items
fnd-pin-off = Always on top
fnd-pin-on = Always on top (on)
fnd-pin-unsupported = Not supported by this desktop
fnd-toast-outcome-unknown = Outcome unknown
fnd-toast-tip-info = Info: { $text }
fnd-toast-tip-success = Success: { $text }
fnd-toast-tip-warning = Warning: { $text }
fnd-toast-tip-error = Error: { $text }
fnd-toast-tip-progress = Progress: { $text }
fnd-empty-clear-filter = Clear filter
fnd-kv-copy-section = Copy section
fnd-kv-copy-row = Copy row
fnd-kv-copy-value = Copy value
fnd-shield-start-failed = Could not start the privileged helper
fnd-shield-auth-cancelled = Authentication cancelled
fnd-props-open-failed = Could not open properties.
fnd-shield-unlock = Unlock privileged actions
fnd-shield-waiting = Waiting for authentication…
fnd-shield-unlocked = Privileged actions unlocked (click to lock)
fnd-table-expand = Expand
fnd-table-collapse = Collapse

## Process windows (E23)

pwn-props-filter-placeholder = Filter
pwn-props-filter-icon-tooltip = Filter
pwn-props-filter-name = Filter { $tab }
pwn-props-filter-no-match = No properties match the current filter.
pwn-props-filter-no-match-threads = No threads match the current filter.

## Performance (E24)

perf-fallback-collecting = Collecting…
perf-fallback-restricted = Restricted
perf-fallback-restricted-tooltip = Needs administrator access.
perf-fallback-needs-setup = Needs setup
perf-fallback-needs-setup-tooltip = Needs setup. Open Additional Setup…
perf-fallback-not-supported = Not supported
perf-fallback-not-supported-tooltip = Not available on this device
perf-fallback-not-reported = Not reported
perf-fallback-unavailable = Unavailable
perf-fallback-could-not-read = Could not read: {$reason}
perf-fallback-unknown = Unknown
perf-last-updated = Last updated {$age} ago
perf-details-heading = Details
perf-header-processor = Processor
perf-header-memory = Memory
perf-header-gpu = Graphics adapter
perf-header-disk = Disk {$n}
perf-header-fan = Fan {$n}
perf-header-battery = Battery
perf-header-disks-count = {$k} disks
perf-header-adapters-count = {$k} adapters
perf-cpu-row-utilization = Utilization
perf-cpu-row-kernel-time = Kernel time
perf-cpu-row-speed = Speed
perf-cpu-row-processes = Processes
perf-cpu-row-threads = Threads
perf-cpu-row-handles = Handles
perf-cpu-row-up-time = Up time
perf-cpu-row-base-speed = Base speed
perf-cpu-row-sockets = Sockets
perf-cpu-row-temperature = Temperature
perf-cpu-row-load-average = Load average
perf-cpu-row-cores = Cores
perf-cpu-row-logical-processors = Logical processors
perf-cpu-row-l1 = L1 cache
perf-cpu-row-l2 = L2 cache
perf-cpu-row-l3 = L3 cache
perf-cpu-tip-fastest = Fastest core: {$mhz} MHz
perf-cpu-tip-bogomips = Reported by the kernel at boot; the current clock is not available
perf-cpu-tip-handles = Open file descriptors, system-wide
perf-cpu-tip-boot = Boot time {$time}
perf-cpu-tip-loadavg = 1, 5 and 15 minutes
perf-card-heading-cpu = CPU
perf-card-heading-memory = Memory
perf-card-heading-gpu = GPU {$n}
perf-card-heading-disk = Disk {$n}
perf-card-heading-fan = Fan {$n}
perf-card-aggregate-storage = Aggregate storage
perf-card-aggregate-graphics = Aggregate graphics
perf-card-multiple = Multiple
perf-card-commit = Commit {$pct}
perf-unknown-processor = Unknown processor
perf-unknown-gpu = Unknown GPU
perf-unknown-adapter = Unknown adapter
perf-net-state-connected = Connected
perf-net-state-disconnected = Disconnected
perf-net-state-testing = Testing
perf-net-state-dormant = Dormant
perf-net-state-not-present = Not present
perf-net-state-lower-layer-down = Lower layer down
perf-net-state-unknown = Unknown
perf-card-heading-battery = Battery
perf-card-disks-count = {$k} disks
perf-card-adapters-count = {$k} adapters
perf-net-type-ethernet = Ethernet
perf-net-type-wifi = Wi-Fi
perf-net-type-vpn = VPN
perf-net-type-virtual = Virtual
perf-net-type-other = Other
perf-cpu-cell = CPU {$n}
perf-cpu-cell-tooltip = Core {$core}, thread {$thread}, {$mhz} MHz
perf-cpu-cell-tooltip-noclock = Core {$core}, thread {$thread}
perf-cpu-offline = Offline
perf-legend-total = Total
perf-legend-kernel = Kernel
perf-cpu-mode-overall = Overall
perf-cpu-mode-per-core = Per core
perf-cpu-mode-overlaid = All cores overlaid
perf-cpu-mode-stacked = All cores stacked
perf-cpu-kernel-on = Kernel on
perf-cpu-kernel-off = Kernel off
perf-cpu-tip-mode = Graph mode
perf-cpu-tip-kernel = Show kernel time
perf-later-version = Available in a later version
perf-cpu-menu-graph = Graph
perf-cpu-menu-kernel-times = Kernel times
perf-cpu-heading-utilization = Utilization
processes-confirm-protected-title = End {$name}?
processes-confirm-protected-text = Ending {$name} will end your desktop session. End it anyway?
processes-confirm-protected-tree-text = Ending {$name} will end your desktop session. End {$count} processes in this tree anyway?
processes-confirm-protected-button = End anyway
processes-confirm-end-tree-title = End process tree?
processes-confirm-end-tree-text = End {$count} processes in this tree?
processes-confirm-end-tree-button = End tree
processes-confirm-kill-app-title = Kill {$app}?
processes-confirm-kill-app-text = Kill all {$count} processes of {$app}?
processes-confirm-kill-app-button = Kill app
processes-confirm-end-many-title = End {$count} processes?
processes-confirm-end-many-text = End {$count} selected processes?
processes-confirm-end-many-kill-text = End {$count} selected processes immediately?
processes-confirm-end-many-button = End tasks
processes-confirm-end-many-kill-button = End processes
processes-confirm-other-user-title = Act on another user's process?
processes-confirm-other-user-text = {$action} on {$name} (PID {$pid}), owned by {$user}. Continue?
processes-confirm-realtime-title = Set realtime priority?
processes-confirm-realtime-text = Realtime priority can freeze the desktop if the process spins. Set it anyway?
processes-confirm-realtime-button = Set realtime
processes-confirm-signal-many-title = Send signal to {$count} processes?
processes-confirm-signal-many-text = Send SIG{$signal} to {$count} selected processes?
processes-confirm-signal-many-button = Send signal
processes-confirm-large-dump-title = Create a large dump?
processes-confirm-large-dump-text = {$name} uses {$size}. The process pauses while the core is written and the file will be about that size. Continue?
processes-confirm-large-dump-button = Create dump
processes-confirm-virustotal-title = Open VirusTotal?
processes-confirm-virustotal-text = This opens virustotal.com with the file's hash. The file is not uploaded.
processes-confirm-virustotal-button = Open
processes-confirm-picked-title = Picked {$name} (PID {$pid})
processes-confirm-picked-text = What do you want to do with this process?
processes-confirm-picked-button = Select
processes-confirm-picked-terminate = Terminate
processes-column-name = Name
processes-column-name-display = Name display
processes-column-image-name = Image name
processes-column-image-path = Image path
processes-column-cmdline = Command line
processes-column-description = Description
processes-column-type = Type
processes-column-start-time = Start time
processes-column-version = Version
processes-column-timestamp = Build date
processes-column-file-mtime = File modified time
processes-column-file-size = File size
processes-column-company = Package
processes-column-signer = Packager
processes-column-signature = Package status
processes-column-architecture = Architecture
processes-column-user = User
processes-column-pid = PID
processes-column-ppid = Parent PID
processes-column-session = Session
processes-column-security-label = Security label
processes-column-elevation = Elevated
processes-column-sandbox = Sandbox
processes-column-priority = Priority
processes-column-affinity = Affinity
processes-column-cpu = CPU
processes-column-cpu-kernel = CPU (kernel)
processes-column-cpu-user = CPU (user)
processes-column-cpu-relative = CPU (one core = 100%)
processes-column-cpu-time = CPU time
processes-column-cpu-time-kernel = CPU time kernel
processes-column-cpu-time-user = CPU time user
processes-column-cycles = Cycles
processes-column-cycles-delta = Cycles delta
processes-column-ctx-switches = Context switches
processes-column-ctx-switches-delta = Context switches delta
processes-column-memory = Memory
processes-column-ws-total = Working set total
processes-column-ws-peak = Working set peak
processes-column-ws-shared = Working set shared
processes-column-commit = Commit size
processes-column-commit-peak = Commit size peak
processes-column-commit-delta = Commit size delta
processes-column-vsize = Virtual size
processes-column-vsize-peak = Virtual size peak
processes-column-page-tables = Page tables
processes-column-page-faults = Page faults
processes-column-page-faults-delta = Page faults delta
processes-column-major-faults = Major faults
processes-column-major-faults-delta = Major faults delta
processes-column-swap = Swap
processes-column-pss = PSS
processes-column-uss = USS
processes-column-io-read-bytes = I/O read bytes
processes-column-io-write-bytes = I/O write bytes
processes-column-io-read-delta = I/O read bytes delta
processes-column-io-write-delta = I/O write bytes delta
processes-column-io-read-rate = I/O read rate
processes-column-io-write-rate = I/O write rate
processes-column-io-read-ops = I/O read ops
processes-column-io-write-ops = I/O write ops
processes-column-disk = Disk
processes-column-disk-read-rate = Disk read rate
processes-column-disk-write-rate = Disk write rate
processes-column-disk-total-rate = Disk total rate
processes-column-disk-read-bytes = Disk read bytes
processes-column-disk-write-bytes = Disk write bytes
processes-column-disk-total-bytes = Disk total bytes
processes-column-disk-read-delta = Disk read bytes delta
processes-column-disk-write-delta = Disk write bytes delta
processes-column-disk-total-delta = Disk total bytes delta
processes-column-disk-reads = Disk reads
processes-column-disk-writes = Disk writes
processes-column-disk-reads-delta = Disk reads delta
processes-column-disk-writes-delta = Disk writes delta
processes-column-network = Network
processes-column-net-send-rate = Network send
processes-column-net-recv-rate = Network receive
processes-column-gpu = GPU
processes-column-gpu-mem-dedicated = VRAM
processes-column-gpu-mem-shared = GPU memory shared
processes-column-gpu-engine = GPU engine
processes-column-gpu-encode = GPU encode
processes-column-gpu-decode = GPU decode
processes-column-gpu-0 = GPU 0
processes-column-gpu-1 = GPU 1
processes-column-gpu-2 = GPU 2
processes-column-gpu-3 = GPU 3
processes-column-gpu-4 = GPU 4
processes-column-gpu-5 = GPU 5
processes-column-gpu-6 = GPU 6
processes-column-gpu-7 = GPU 7
processes-column-gpu-8 = GPU 8
processes-column-gpu-9 = GPU 9
processes-column-gpu-10 = GPU 10
processes-column-gpu-11 = GPU 11
processes-column-gpu-12 = GPU 12
processes-column-gpu-13 = GPU 13
processes-column-gpu-14 = GPU 14
processes-column-gpu-15 = GPU 15
processes-column-power = Power usage
processes-column-power-trend = Power usage trend
processes-column-handles = Handles
processes-column-threads = Threads
processes-column-windows = Windows
processes-column-x11-resources = X11 resources
processes-column-state = State
processes-column-nice = Nice
processes-column-sched-policy = Scheduling policy
processes-column-io-priority = I/O priority
processes-column-unit = Unit
processes-column-cgroup = Cgroup
processes-column-app-id = App ID
processes-column-oom-score = OOM score
processes-column-oom-score-adj = OOM adj
processes-column-tty = TTY
processes-column-cwd = Working directory
processes-column-pgrp = Process group
processes-column-sid = SID
processes-column-last-cpu = Last CPU
processes-column-container = Container
processes-column-memory-alias = Working set private
processes-column-gpu-mem-dedicated-alias = GPU memory dedicated
processes-column-memory-tooltip = Private memory (RssAnon). Shared and file-backed pages are not counted.
processes-column-ws-total-tooltip = Resident set size (VmRSS)
processes-column-network-tooltip = Attributed TCP traffic. UDP and other users' sockets are not counted.
processes-column-handles-tooltip = Open file descriptors
processes-column-visible-rows-tooltip = Only measured for visible rows
processes-group-image = Image
processes-group-identity = Identity
processes-group-security = Security
processes-group-scheduling = Scheduling
processes-group-cpu = CPU
processes-group-memory = Memory
processes-group-io = I/O
processes-group-disk = Disk
processes-group-network = Network
processes-group-gpu = GPU
processes-group-power = Power
processes-group-objects = Objects
processes-group-linux = Linux
processes-type-app = App
processes-type-unit = Unit
processes-type-background = Background process
processes-type-system = System process
processes-type-kernel-thread = Kernel thread
processes-type-pseudo = Pseudo
processes-state-running = Running
processes-state-sleeping = Sleeping
processes-state-disk-sleep = Disk sleep
processes-state-stopped = Stopped
processes-state-zombie = Zombie
processes-state-idle = Idle
processes-state-dead = Dead
processes-state-frozen = Frozen
processes-name-suspended = Suspended
processes-name-frozen = Frozen
processes-priority-realtime = Realtime
processes-priority-idle = Idle
processes-priority-high = High
processes-priority-above-normal = Above normal
processes-priority-normal = Normal
processes-priority-below-normal = Below normal
processes-priority-low = Low
processes-priority-batch = batch
processes-affinity-all = All
processes-header-apps = Apps
processes-header-background = Background processes
processes-header-system = System processes
shell-titlebar-minimize = Minimize
shell-titlebar-maximize = Maximize
shell-titlebar-restore = Restore
shell-titlebar-close = Close
shell-tab-processes = Processes
shell-tab-history = History
shell-tab-performance = Performance
shell-tab-services = Services
shell-tab-startup = Startup Apps
shell-tab-connections = Connections
shell-tab-information = Information
shell-status-paused = [PAUSED - Ctrl]

## Charts (E19)

chart-grid-offline = Offline

## Table header menu (E18)

table-header-menu-sort-by = Sort by { $label }
table-header-menu-select-columns = Select Columns…
table-header-menu-hide = Hide This Column
table-header-menu-hide-tooltip = Hide { $label }
table-header-menu-fit = Resize columns to fit content
table-header-menu-resize-behavior = Resize Behavior
table-header-menu-proportional = Proportional
table-header-menu-last-column = Last Column
table-header-menu-reset = Reset to default
table-toast-last-visible = At least one column must stay visible
table-tooltip-name-not-hideable = Name cannot be hidden
table-toast-columns-reset = Columns reset to default

# TlKeyValueGrid state words (PERF-013)
fnd-kv-collecting = Collecting…
fnd-kv-restricted = Restricted
fnd-kv-unavailable = Unavailable
fnd-kv-not-supported = Not supported
fnd-kv-not-reported = Not reported
fnd-kv-needs-setup = Needs setup
# CPU Details cache and boot time tooltips
perf-cpu-tip-l1-data = {$size} data
perf-cpu-tip-l1-instruction = {$size} instruction
perf-cpu-tip-l4 = L4 {$size}
perf-cpu-time-unknown = Unknown
# System table status labels (SYS-003, SYS-020)
sys-sys003-running = Running
sys-sys003-active = Active
sys-sys003-starting = Starting
sys-sys003-stopping = Stopping
sys-sys003-reloading = Reloading
sys-sys003-stopped = Stopped
sys-sys003-failed = Failed
sys-sys003-maintenance = Maintenance
sys-sys003-enabled = Enabled
sys-sys003-disabled = Disabled
sys-sys003-masked = Masked
sys-sys003-not-in-this-desktop = Not in this desktop
sys-sys003-missing-program = Missing program

# TlKeyValueGrid empty filter text
fnd-kv-no-match = No properties match the current filter.

# Column picker window (UI-PRC-007, R-106)
table-picker-title = tasklens Columns
table-picker-filter-placeholder = Filter columns…
table-picker-always-on = always on
table-picker-na = n/a on this system
table-picker-no-match = No columns match
table-picker-count = { $shown } of { $total } columns shown
table-picker-reset = Reset to default
table-picker-close = Close
table-picker-name-processes = Processes table
table-picker-name-history = History table
table-picker-name-services = Services table
table-picker-name-startup = Startup Apps table
table-picker-name-connections = Connections table

# Low-memory rescue window (UI-PWN-032, E32-009)
lowmem-window-title = tasklens - Low memory
lowmem-text-critical = Your device is running on critically low memory.
lowmem-text-recommend = We recommend you close some of the processes below.
lowmem-text-eased = Memory pressure has eased. You can open the full tasklens or close this window.
lowmem-filter-placeholder = Filter...
lowmem-filter-empty = No processes match the filter.
lowmem-button-open-full = Open Full tasklens

# Share Theme window (UI-SHL-082)
shell-share-theme-title = tasklens Share Theme
shell-share-theme-text = Copy this code and send it to someone who uses tasklens.
shell-share-theme-copy = Copy
shell-share-theme-close = Close
shell-toast-theme-code-copied = Theme code copied

# Collector Health window (UI-SHL-099)
shell-health-title = tasklens Collector Health
shell-health-col-source = Source
shell-health-col-state = State
shell-health-col-last-data = Last data
shell-health-col-restarts = Restarts
shell-health-col-last-error = Last error
shell-health-state-running = Running
shell-health-state-late = Late
shell-health-state-stale = Stale
shell-health-state-failed = Failed
shell-health-state-disabled = Disabled
shell-health-state-unsupported = Unsupported
# $age is a short age such as "3s" or "12m 5s".
shell-health-ago = { $age } ago
# Source names, in SourceId order.
shell-health-source-cpu = CPU
shell-health-source-memory = Memory
shell-health-source-disk = Disks
shell-health-source-net = Network
shell-health-source-gpu-nvidia = GPU (NVIDIA)
shell-health-source-gpu-drm = GPU (DRM)
shell-health-source-sensors = Sensors
shell-health-source-processes = Processes
shell-health-source-process-detail = Process details
shell-health-source-connections = Connections
shell-health-source-systemd-system = Services (system)
shell-health-source-systemd-user = Services (user)
shell-health-source-logind = Sessions
shell-health-source-startup = Startup
shell-health-source-inventory = Inventory
shell-health-source-kwin = KWin
shell-health-source-udisks = Storage devices
shell-health-source-upower = Power
shell-health-source-network-manager = NetworkManager
shell-health-source-history = History
shell-health-source-journal = Journal

## tl-app (E33)

app-114-tasklens-stopped-after-an-unrecoverable = tasklens stopped after an unrecoverable error.
app-114-tasklens-could-not-start-the = tasklens could not start the renderer
app-114-tasklens-lost-the-graphics-device = tasklens lost the graphics device and must restart

## Shell filter box (E20-009)

shell-filter-label = Filter:
shell-filter-clear = Clear filter
shell-filter-name = Filter

## Processes toolbar buttons (E22, UI-PRC-019)

processes-toolbar-hide-system = Hide system processes
processes-toolbar-show-system = Show system processes
processes-toolbar-mode-tree = Tree View
processes-toolbar-mode-grouped = Task Manager Grouping
processes-toolbar-mode-flat = List View
processes-toolbar-cycle-mode = Toggle view mode (current: {$mode})
processes-toolbar-history-flat = History shows a flat list

## Processes table toasts (E22, UI-PRC-011)

processes-toast-process-exited = Process exited

## Include children checkbox (E20-010)

shell-filter-include-children = Include children

## Process row tooltip (PROC-039)

processes-tooltip-path = path:
processes-tooltip-restricted = restricted
processes-tooltip-user = user:
processes-tooltip-uid = uid
processes-tooltip-unit = Unit:
processes-tooltip-cgroup = cgroup:
processes-tooltip-desktop-file = Desktop file:
processes-tooltip-some-values-need-administrator-access = Some values need administrator access.
processes-tooltip-cpu-time-spent-in-hardware = CPU time spent in hardware and software interrupts
processes-tooltip-memory-held-by-zram-compressed = Memory held by zram compressed swap devices
processes-cell-tooltip-app-memory = Anonymous memory charged to this app's cgroup
processes-cell-tooltip-nice = nice { $nice }
processes-cell-tooltip-power-estimate = ≈ { $watts } W (estimate)
processes-cell-tooltip-power-needs-setup = Needs setup
processes-cell-tooltip-power-no-sensor = No power sensor available
processes-cell-tooltip-zram = original { $original }, compressed { $compressed }, ratio { $ratio }
processes-cell-tooltip-oom-score-adj = oom_score_adj { $adj }
processes-cell-tooltip-disk-ops = Linux counts storage operations per cgroup, not per process
processes-cell-tooltip-irq-in-task-time = Interrupt time is included in task time on this kernel

# UI scale keys (E20-013)
shell-toast-ui-scale = UI scale changed to { $pct }%

# History mode page (E30-016)
history-header-collecting = Collecting first history snapshot...
history-header-live = History - Drag scrubber below to view timeline
history-header-snapshot = History Snapshot: { $count } processes - { $min } min { $sec } sec ago
history-header-snapshot-sec = History Snapshot: { $count } processes - { $sec } sec ago
history-header-snapshot-hour = History Snapshot: { $count } processes - { $hours } h { $min } min ago
history-header-range = History Range: { $from } - { $to } ({ $duration }), averages
history-duration = { $min } min { $sec } sec
history-duration-sec = { $sec } sec
history-duration-hour = { $hours } h { $min } min
history-loading = Loading history…

# Keyboard Shortcuts window (E21-016)
shortcuts-title = tasklens Keyboard Shortcuts
shortcuts-search-placeholder = Search shortcuts
shortcuts-or = or
shortcuts-heading-global = Global
shortcuts-heading-tables = Tables
shortcuts-heading-processes = Processes
shortcuts-heading-history = History
shortcuts-heading-services = Services
shortcuts-heading-startup-apps = Startup Apps
shortcuts-heading-connections = Connections
shortcuts-heading-information = Information
shortcuts-heading-performance = Performance
shortcuts-heading-properties-windows = Properties windows
shortcuts-heading-dialogs = Dialogs
shortcuts-key-hold-ctrl = Hold Ctrl
shortcuts-key-printable = Printable character
shortcuts-key-shift-click = Shift+click
shortcuts-key-ctrl-click = Ctrl+click
shortcuts-select-tab = Select tab 1 to 6 (Processes, Performance, Services, Startup Apps, Connections, Information)
shortcuts-next-tab = Next / previous tab
shortcuts-open-run-dialog = Run…
shortcuts-quit = Exit tasklens
shortcuts-close-window = Close window (follows close behaviour)
shortcuts-open-settings = All settings…
shortcuts-open-shortcuts = Keyboard Shortcuts
shortcuts-open-file-menu = Open File menu
shortcuts-open-menu = Open File / View / Settings / Help menu
shortcuts-open-debug-menu = Open Debug menu
shortcuts-focus-filter = Focus filter
shortcuts-refresh-current-tab = Refresh current tab now
shortcuts-escape-chain = Close topmost dialog or menu; else clear filter and focus the table; else close window if Close with Esc is on
shortcuts-pause-display = Pause display
shortcuts-scale = UI scale +5 / −5 / 100
shortcuts-toggle-mini-window = Performance mini window
shortcuts-move-focus = Move row selection
shortcuts-extend-selection = Extend or toggle selection
shortcuts-select-all = Select all visible rows
shortcuts-expand-or-child = Expand, or move to first child / collapse, or move to parent
shortcuts-toggle-expand = Toggle expand
shortcuts-open-properties = Properties
shortcuts-copy-row = Copy row
shortcuts-copy-rows-as-table = Copy rows as table (TSV with header)
shortcuts-collapse-all = Collapse all
shortcuts-open-context-menu = Open context menu for the selection
shortcuts-start-filtering = Start filtering
shortcuts-end-task = End Task (SIGTERM)
shortcuts-end-process = End Process (SIGKILL)
shortcuts-copy-details = Copy details
shortcuts-perf-copy-device-info = Copy device info
shortcuts-perf-toggle-summary-mode = Graph Summary View
shortcuts-perf-toggle-sidebar = Toggle device sidebar
shortcuts-perf-select-visible-device = Select visible device 1 to 8
shortcuts-perf-move-selection = Move selection
shortcuts-props-select-tab = Select tab 1 to 7
shortcuts-props-next-tab = Next / previous tab
shortcuts-props-focus-filter = Focus the active tab's filter
shortcuts-props-refresh-now = Refresh the active tab
shortcuts-props-field = Move between fields, activate a link, copy the field
shortcuts-affinity = Move between CPU tiles and toggle
shortcuts-priority = Slider steps 1 and 5, ends, stepper, Set priority
shortcuts-signal = Step the signal number, Send signal
shortcuts-run = History pick, Run, Run as administrator
shortcuts-cancel-window-pick = Cancel window picking
shortcuts-column-picker = Move, toggle, jump
shortcuts-handle-search = Start search; stop a running search, else close when idle
shortcuts-handle-search-table = End Task, End Process, Go to process, copy paths, copy rows as TSV
shortcuts-low-memory = Clear filter then close; close; start filtering

# Status chip health items (E20-016)
shell-health-stale = { $source } data stale
shell-health-failed = { $source } unavailable
shell-health-disabled = { $source } paused after errors
shell-health-more = +{ $n } more
shell-health-no-data = No new data for { $secs } s
shell-health-unknown-error = Unknown error
shell-health-restarting = Restarting in a minute
shell-health-open = Open Collector Health
shell-health-restricted = Restricted: { $n } processes
shell-health-chip-gpu = GPU
shell-health-chip-services = Services
