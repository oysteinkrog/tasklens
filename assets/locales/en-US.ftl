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

## Charts (E19)

chart-grid-offline = Offline
