//! Host settings in the window (ADR-018 Settings, tasklens-q4x76.23): `processes.interval_ms`
//! and the `[history]` table belong to the host's engine, not to `settings.toml`.
//!
//! The window reads them only from the newest snapshot of the host (`Snapshot.host_settings`),
//! so there is one source and no copy to go stale. Until the engine has given them (revision 0)
//! the readers see the default values and the controls that change them stay disabled, so the
//! window never writes values it has not read. A change goes to the host's engine with the
//! revision it was made from; on a conflict nothing changes here and the next snapshot shows the
//! current values. `TlSettings::set` and `TlSettings::toggle` send the host keys here, and
//! `TlSettings` never writes them to `settings.toml`.
//!
//! A change also carries the hub's generation, the one its revision came with. After the window
//! switched engines or reconnected, the client refuses a change made from the older snapshot,
//! since two engines can have the same revision (ADR-018 Generations). Every control reads the
//! hub's installed snapshot through `current`. While that snapshot is from an older generation
//! than the hub's client (after a switch or a reconnect, before the new engine's first snapshot
//! is installed) `current` gives revision 0, so the controls are disabled until the new engine's
//! host settings arrive, as in a fresh window.

use std::sync::Arc;

use gpui_kit::{App, AsyncApp};
use tl_model::client::EngineClient;
use tl_model::remote_proto::{
    HostSettings, HostSettingsChanges, HostSettingsValues, SetHostSettingsReply,
};
use tracing::{debug, info, warn};

use crate::hub::{GlobalHub, SnapshotHub};
use crate::settings_global::{SettingKey, SettingValue};

/// The `SettingKey`s whose values are host settings.
pub const HOST_KEYS: [SettingKey; 5] = [
    SettingKey::ProcessesIntervalMs,
    SettingKey::HistoryEnabled,
    SettingKey::HistoryRetentionH,
    SettingKey::HistoryPersist,
    SettingKey::HistoryMaxDiskMb,
];

/// True for a key whose value is a host setting.
pub fn is_host_key(key: SettingKey) -> bool {
    HOST_KEYS.contains(&key)
}

/// The host settings in the newest snapshot. Revision 0 with the default values before the
/// engine gave them, and without a hub. Revision 0 with the last values shown while the hub's
/// snapshot is from an older generation than its client, so nothing enables or writes from a
/// session that has ended.
pub fn current(cx: &App) -> Arc<HostSettings> {
    cx.try_global::<GlobalHub>()
        .map(|hub| installed(hub.0.read(cx)))
        .unwrap_or_default()
}

/// The host settings of `hub`'s snapshot, with revision 0 while that snapshot's generation is
/// not the client's current one (ADR-018 Generations).
fn installed(hub: &SnapshotHub) -> Arc<HostSettings> {
    let host = &hub.current().host_settings;
    if host.rev == 0 || hub.generation() == hub.client().generation() {
        return Arc::clone(host);
    }
    Arc::new(HostSettings {
        rev: 0,
        ..HostSettings::clone(host)
    })
}

/// The hub's client, its generation and the host settings in its snapshot, from one read.
fn hub_state(cx: &App) -> Option<(Arc<dyn EngineClient>, u64, Arc<HostSettings>)> {
    cx.try_global::<GlobalHub>().map(|hub| {
        let hub = hub.0.read(cx);
        (Arc::clone(hub.client()), hub.generation(), installed(hub))
    })
}

/// The values to show and use. The defaults until the engine gave them.
pub fn values(cx: &App) -> HostSettingsValues {
    current(cx).values.clone()
}

/// The engine gave the host settings, so the controls that change them are enabled.
pub fn known(cx: &App) -> bool {
    current(cx).rev > 0
}

/// The value of a host key, as `TlSettings` would hold it. `None` for any other key.
pub fn read_key(values: &HostSettingsValues, key: SettingKey) -> Option<SettingValue> {
    let h = &values.history;
    Some(match key {
        SettingKey::ProcessesIntervalMs => SettingValue::Int(values.interval_ms.into()),
        SettingKey::HistoryEnabled => SettingValue::Bool(h.enabled),
        SettingKey::HistoryRetentionH => SettingValue::Int(h.retention_h.into()),
        SettingKey::HistoryPersist => SettingValue::Bool(h.persist),
        SettingKey::HistoryMaxDiskMb => SettingValue::Int(h.max_disk_mb.into()),
        _ => return None,
    })
}

/// Writes `value` to the host key `key`. A kind mismatch, an integer that does not fit or a key
/// that is not a host key logs a warning and sends nothing.
pub fn set(cx: &mut App, key: SettingKey, value: &SettingValue) {
    set_then(cx, key, value, |_| {});
}

/// As `set`, and calls `on_ack` once the engine applied the change (see `change_then`).
pub fn set_then(
    cx: &mut App,
    key: SettingKey,
    value: &SettingValue,
    on_ack: impl FnOnce(&mut App) + 'static,
) {
    let mut rejected = None;
    change_then(
        cx,
        |v| {
            if let Err(why) = assign(v, key, value) {
                rejected = Some(why);
            }
        },
        on_ack,
    );
    if let Some(why) = rejected {
        warn!(target: "tasklens::settings", key = key.as_str(), why, "set rejected");
    }
}

/// Flips the Bool host key `key`.
pub fn toggle(cx: &mut App, key: SettingKey) {
    if let Some(SettingValue::Bool(on)) = read_key(&current(cx).values, key) {
        set(cx, key, &SettingValue::Bool(!on));
    }
}

/// Applies `f` to the current values and sends what changed to the host's engine, with the
/// revision they came from. Sends nothing before the engine gave them, or when nothing changed.
pub fn change(cx: &mut App, f: impl FnOnce(&mut HostSettingsValues)) {
    change_then(cx, f, |_| {});
}

/// As `change`, and calls `on_ack` once the engine replied that it applied the change. It is
/// not called when nothing was sent (not read yet, no change, a full queue, no engine), on a
/// conflict, or when the connection dropped before the reply.
pub fn change_then(
    cx: &mut App,
    f: impl FnOnce(&mut HostSettingsValues),
    on_ack: impl FnOnce(&mut App) + 'static,
) {
    // Without a hub nothing was read, so there is nothing to change from.
    let Some((client, generation, current)) = hub_state(cx) else {
        return;
    };
    if current.rev == 0 {
        debug!(target: "tasklens::settings", "host settings not read yet; change dropped");
        return;
    }
    let mut next = current.values.clone();
    f(&mut next);
    next.clamp();
    let changes = HostSettingsChanges {
        interval_ms: (next.interval_ms != current.values.interval_ms).then_some(next.interval_ms),
        history: (next.history != current.values.history).then(|| next.history.clone()),
        // Only `tasklens-setup` turns remote admin on or off.
        remote_admin: None,
    };
    if changes == HostSettingsChanges::default() {
        return;
    }
    match client.set_host_settings(generation, current.rev, changes) {
        Ok(reply) => {
            // The snapshot carries the values either way; a conflict or a lost reply is only
            // logged.
            cx.spawn(async move |cx: &mut AsyncApp| match reply.recv().await {
                Ok(SetHostSettingsReply::Ack { rev }) => {
                    debug!(target: "tasklens::settings", rev, "host settings changed");
                    cx.update(on_ack);
                }
                Ok(SetHostSettingsReply::Conflict { current }) => {
                    info!(target: "tasklens::settings", rev = current.rev, "host settings changed elsewhere; change not applied");
                }
                Err(_) => {
                    info!(target: "tasklens::settings", "no reply to the host settings change");
                }
            })
            .detach();
        }
        Err(e) => warn!(target: "tasklens::settings", "host settings not sent: {e:?}"),
    }
}

/// Writes `value` to the field of `key` in `v`.
fn assign(
    v: &mut HostSettingsValues,
    key: SettingKey,
    value: &SettingValue,
) -> Result<(), &'static str> {
    let h = &mut v.history;
    match (key, value) {
        (SettingKey::ProcessesIntervalMs, SettingValue::Int(i)) => int(&mut v.interval_ms, *i),
        (SettingKey::HistoryEnabled, SettingValue::Bool(b)) => {
            h.enabled = *b;
            Ok(())
        }
        (SettingKey::HistoryRetentionH, SettingValue::Int(i)) => int(&mut h.retention_h, *i),
        (SettingKey::HistoryPersist, SettingValue::Bool(b)) => {
            h.persist = *b;
            Ok(())
        }
        (SettingKey::HistoryMaxDiskMb, SettingValue::Int(i)) => int(&mut h.max_disk_mb, *i),
        _ if !is_host_key(key) => Err("not a host key"),
        _ => Err("kind mismatch"),
    }
}

fn int<T: TryFrom<i64>>(field: &mut T, v: i64) -> Result<(), &'static str> {
    *field = T::try_from(v).map_err(|_| "integer out of range")?;
    Ok(())
}
