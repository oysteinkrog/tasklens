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

use std::sync::Arc;

use gpui_kit::App;
use tl_model::client::EngineClient;
use tl_model::remote_proto::{
    HostSettings, HostSettingsChanges, HostSettingsValues, SetHostSettingsReply,
};
use tracing::{debug, info, warn};

use crate::app::EngineGlobal;
use crate::hub::GlobalHub;
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
/// engine gave them, and without a hub.
pub fn current(cx: &App) -> Arc<HostSettings> {
    cx.try_global::<GlobalHub>()
        .map(|hub| Arc::clone(&hub.0.read(cx).current().host_settings))
        .unwrap_or_default()
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
    let mut rejected = None;
    change(cx, |v| {
        if let Err(why) = assign(v, key, value) {
            rejected = Some(why);
        }
    });
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
    let current = current(cx);
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
    let Some(client) = client(cx) else {
        return;
    };
    match client.set_host_settings(current.rev, changes) {
        Ok(reply) => {
            cx.background_executor()
                .spawn(async move {
                    // The snapshot carries the result either way; a conflict is only logged.
                    if let Ok(SetHostSettingsReply::Conflict { current }) = reply.recv().await {
                        info!(target: "tasklens::settings", rev = current.rev, "host settings changed elsewhere; change not applied");
                    }
                })
                .detach();
        }
        Err(e) => warn!(target: "tasklens::settings", "host settings not sent: {e:?}"),
    }
}

/// The client of the host the hub shows, else the app's engine client.
fn client(cx: &App) -> Option<Arc<dyn EngineClient>> {
    cx.try_global::<GlobalHub>()
        .map(|hub| Arc::clone(hub.0.read(cx).client()))
        .or_else(|| cx.try_global::<EngineGlobal>().map(|e| Arc::clone(&e.0)))
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
