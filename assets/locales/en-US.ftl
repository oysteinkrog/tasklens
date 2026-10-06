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
