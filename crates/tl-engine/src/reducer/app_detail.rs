//! The App General tab payload of Properties windows (PROC-085, tasklens-dwy.12).
//!
//! The reducer builds `TabPayload::AppGeneral` from the process snapshot for each
//! `DetailTarget::App(GroupKey)` in `Demand.details`. No worker reads anything for it. A group's
//! payload is rebuilt on every refresh, but the old `Arc` stays when the new value is equal, so an
//! unchanged group is pointer-equal across publishes. A group with no live member keeps its last
//! payload and gets `Validity::Missing`, which the UI shows as the exited state.
//!
//! Privacy: the command line is user data. It is never logged here (ADR-015).

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_model::details::{AppGeneralDetail, AppKind, DetailTab, DetailTarget, TabPayload};
use tl_model::ids::GroupKey;
use tl_model::process::{AppCgroupSample, ProcFlags, ProcessRecord, is_row_member};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::table::{Total, UnitRef};
use tl_model::validity::{ProcessField, Validity};

use super::DetailsState;
use super::table::unit_stat;
use crate::update::CgroupStats;

/// Builds the General payload of group `key`. `None` when the group has no live member.
pub(crate) fn build(
    key: GroupKey,
    snap: &ProcessSnapshot,
    unit_stats: &[CgroupStats],
    unit_names: &FxHashMap<UnitRef, Arc<str>>,
) -> Option<AppGeneralDetail> {
    let members: Vec<(&ProcessRecord, Option<&UnitRef>)> = snap
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.flags.contains(ProcFlags::EXITED))
        .filter(|(i, r)| is_row_member(r, snap.extras.get(*i), key))
        .map(|(i, r)| (r, snap.extras.get(i).and_then(|x| x.unit.as_ref())))
        .collect();
    // The primary process carries the app id; otherwise the oldest member.
    let primary = members
        .iter()
        .min_by_key(|(r, _)| (r.meta.app_id.is_none(), r.key))
        .copied()?;
    let (primary, primary_unit) = primary;
    let is_unit = primary.group != key;
    let unit = primary_unit.cloned();

    let sample: Option<&AppCgroupSample> = if is_unit {
        None
    } else {
        snap.apps
            .binary_search_by(|s| s.group.cmp(&key))
            .ok()
            .map(|i| &snap.apps[i])
    };
    let unit_sample = if is_unit {
        unit.as_ref().and_then(|u| unit_stat(unit_stats, u))
    } else {
        None
    };

    let (anon, swap) = match (sample, unit_sample) {
        (Some(s), _) => (
            Total {
                value: s.anon_bytes,
                validity: s.validity.get(ProcessField::RssAnon),
            },
            Total {
                value: s.swap_bytes,
                validity: s.validity.get(ProcessField::Swap),
            },
        ),
        (None, Some(u)) => (
            Total {
                value: u.anon_bytes,
                validity: u.validity,
            },
            Total {
                value: u.swap_bytes,
                validity: u.validity,
            },
        ),
        (None, None) => (Total::default(), Total::default()),
    };

    let name: Arc<str> = if is_unit {
        unit.as_ref()
            .map(|u| unit_names.get(u).cloned().unwrap_or_else(|| u.name.clone()))
            .unwrap_or_else(|| primary.meta.name.clone())
    } else {
        primary.meta.name.clone()
    };
    let cgroup = sample
        .map(|s| s.cgroup.rel_path.clone())
        .or_else(|| unit_sample.map(|s| s.cgroup.rel_path.clone()))
        .or_else(|| primary.meta.cgroup.clone());
    let processes = sample
        .map(|s| s.pids)
        .or(unit_sample.map(|s| s.pids_current))
        .unwrap_or(members.len() as u32);

    Some(AppGeneralDetail {
        name,
        kind: if is_unit { AppKind::Unit } else { AppKind::App },
        icon_path: primary.meta.icon.clone(),
        app_id: if is_unit {
            None
        } else {
            primary.meta.app_id.clone()
        },
        desktop_file: if is_unit {
            None
        } else {
            primary.meta.desktop_path.clone()
        },
        unit,
        cgroup,
        processes,
        memory_anon_bytes: anon,
        swap_bytes: swap,
        disk_read_bps: sample
            .map(|s| Total {
                value: f64::from(s.io_read_bps),
                validity: s.validity.get(ProcessField::IoRead),
            })
            .unwrap_or_default(),
        disk_write_bps: sample
            .map(|s| Total {
                value: f64::from(s.io_write_bps),
                validity: s.validity.get(ProcessField::IoWrite),
            })
            .unwrap_or_default(),
        primary_cmdline: match &primary.meta.cmdline {
            Some(c) => Total {
                value: c.clone(),
                validity: Validity::Valid,
            },
            None => Total {
                value: Arc::from(""),
                validity: Validity::Restricted,
            },
        },
    })
}

/// Refreshes the General tab of every group in `groups`. Applies a tab only when its value or
/// validity changed, so the `Arc` and the version stay put otherwise.
pub(crate) fn refresh(
    details: &mut DetailsState,
    groups: &[GroupKey],
    snap: &ProcessSnapshot,
    unit_stats: &[CgroupStats],
    unit_names: &FxHashMap<UnitRef, Arc<str>>,
) {
    for &key in groups {
        let target = DetailTarget::App(key);
        let slot = details
            .targets
            .get(&target)
            .map(|t| t.tab(DetailTab::AppGeneral).clone());
        let old = slot.as_ref().and_then(|s| match s.payload.as_deref() {
            Some(TabPayload::AppGeneral(d)) => Some(Arc::clone(d)),
            _ => None,
        });
        let old_validity = slot.map(|s| s.validity);
        match build(key, snap, unit_stats, unit_names) {
            Some(new) => {
                if old.as_deref() == Some(&new) && old_validity == Some(Validity::Valid) {
                    continue;
                }
                let payload = match old {
                    Some(old) if *old == new => old,
                    _ => Arc::new(new),
                };
                details.apply_tab(
                    target,
                    DetailTab::AppGeneral,
                    Arc::new(TabPayload::AppGeneral(payload)),
                    Validity::Valid,
                );
            }
            None => {
                // Exited. With no earlier payload there is nothing to keep showing.
                if let Some(old) = old
                    && old_validity != Some(Validity::Missing)
                {
                    details.apply_tab(
                        target,
                        DetailTab::AppGeneral,
                        Arc::new(TabPayload::AppGeneral(old)),
                        Validity::Missing,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::actions::{CgroupRef, SchedPolicy};
    use tl_model::ids::ProcessKey;
    use tl_model::process::{GroupKind, ProcState, ProcessExtras, ProcessMeta};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
    use tl_model::system::CpuSet;
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;

    fn app_key() -> GroupKey {
        GroupKind::App {
            app_id: "firefox".into(),
        }
        .key()
    }

    fn rec(pid: u32, primary: bool, flags: ProcFlags) -> ProcessRecord {
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(pid),
                boot_id: BootId(1),
                pid,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: "firefox".into(),
                name: "Firefox".into(),
                exe: None,
                cmdline: Some(format!("/usr/bin/firefox --pid {pid}").into()),
                user: "user".into(),
                cgroup: Some("user.slice/app.slice/app-firefox.scope".into()),
                app_id: primary.then(|| "firefox".into()),
                icon: Some("/icons/firefox.png".into()),
                description: None,
                desktop_path: primary.then(|| "/usr/share/applications/firefox.desktop".into()),
                search_lower: "firefox".into(),
            }),
            group: app_key(),
            state: ProcState::Sleeping,
            flags,
            nice: 0,
            threads: 1,
            cpu_pct: 1.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn extras() -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            start_boot_ns: 0,
            rss_peak_bytes: 0,
            rss_shared_bytes: 0,
            commit_bytes: 0,
            vsize_bytes: 0,
            vsize_peak_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::empty(),
            handles: None,
            exe_name: "firefox".into(),
            exe_path: "/usr/bin/firefox".into(),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
            pgrp: 0,
            sid: 0,
            last_cpu: 0,
        }
    }

    fn sample(anon: u64) -> AppCgroupSample {
        AppCgroupSample {
            group: app_key(),
            anon_bytes: anon,
            swap_bytes: 4096,
            io_read_bps: 10.0,
            io_write_bps: 20.0,
            rios: 0,
            wios: 0,
            pids: 7,
            frozen: false,
            cgroup: CgroupRef {
                rel_path: "user.slice/app.slice/app-firefox.scope".into(),
                cgroup_id: 1,
            },
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn snapshot(records: Vec<ProcessRecord>, apps: Vec<AppCgroupSample>) -> ProcessSnapshot {
        let extras: Vec<ProcessExtras> = records.iter().map(|_| extras()).collect();
        ProcessSnapshot {
            header: ComponentHeader::default(),
            records: records.into(),
            coverage: Default::default(),
            extras: extras.into(),
            apps: apps.into(),
            extras_p2: Arc::from(Vec::new()),
            low_memory: None,
        }
    }

    fn live(anon: u64) -> ProcessSnapshot {
        snapshot(
            vec![
                rec(10, false, ProcFlags::empty()),
                rec(11, true, ProcFlags::empty()),
            ],
            vec![sample(anon)],
        )
    }

    fn run(details: &mut DetailsState, snap: &ProcessSnapshot) {
        refresh(details, &[app_key()], snap, &[], &FxHashMap::default());
    }

    fn slot(details: &DetailsState) -> tl_model::details::TabSlot {
        details.targets[&DetailTarget::App(app_key())]
            .tab(DetailTab::AppGeneral)
            .clone()
    }

    fn detail(details: &DetailsState) -> Arc<AppGeneralDetail> {
        match slot(details).payload.as_deref() {
            Some(TabPayload::AppGeneral(d)) => Arc::clone(d),
            other => panic!("not AppGeneral: {other:?}"),
        }
    }

    #[test]
    fn fields_come_from_the_snapshot() {
        let mut details = DetailsState::default();
        run(&mut details, &live(1 << 30));
        let d = detail(&details);
        assert_eq!(&*d.name, "Firefox");
        assert_eq!(d.kind, AppKind::App);
        assert_eq!(d.app_id.as_deref(), Some("firefox"));
        assert_eq!(
            d.desktop_file.as_deref(),
            Some("/usr/share/applications/firefox.desktop")
        );
        assert_eq!(d.icon_path.as_deref(), Some("/icons/firefox.png"));
        assert_eq!(
            d.cgroup.as_deref(),
            Some("user.slice/app.slice/app-firefox.scope")
        );
        assert_eq!(d.processes, 7);
        assert_eq!(d.memory_anon_bytes.value, 1 << 30);
        assert_eq!(d.memory_anon_bytes.validity, Validity::Valid);
        assert_eq!(d.swap_bytes.value, 4096);
        assert_eq!(d.disk_read_bps.value, 10.0);
        assert_eq!(d.disk_write_bps.value, 20.0);
        // The primary process is the one with the app id (pid 11), not the oldest.
        assert_eq!(&*d.primary_cmdline.value, "/usr/bin/firefox --pid 11");
        assert_eq!(slot(&details).validity, Validity::Valid);
    }

    #[test]
    fn arc_is_reused_when_nothing_changed() {
        let mut details = DetailsState::default();
        run(&mut details, &live(1 << 30));
        let first = slot(&details);
        run(&mut details, &live(1 << 30));
        let second = slot(&details);
        assert!(Arc::ptr_eq(
            first.payload.as_ref().unwrap(),
            second.payload.as_ref().unwrap()
        ));
        assert_eq!(first.version, second.version);

        run(&mut details, &live(2 << 30));
        let third = slot(&details);
        assert!(!Arc::ptr_eq(
            first.payload.as_ref().unwrap(),
            third.payload.as_ref().unwrap()
        ));
        assert_eq!(detail(&details).memory_anon_bytes.value, 2 << 30);
    }

    #[test]
    fn group_gone_gives_exited() {
        let mut details = DetailsState::default();
        run(&mut details, &live(1 << 30));
        let before = detail(&details);

        // Every member exited: the last payload stays, validity is Missing.
        let exited = snapshot(
            vec![
                rec(10, false, ProcFlags::EXITED),
                rec(11, true, ProcFlags::EXITED),
            ],
            vec![],
        );
        run(&mut details, &exited);
        assert_eq!(slot(&details).validity, Validity::Missing);
        assert!(Arc::ptr_eq(&before, &detail(&details)));
        let version = slot(&details).version;

        // Gone from the snapshot altogether: no further change.
        run(&mut details, &snapshot(vec![], vec![]));
        assert_eq!(slot(&details).validity, Validity::Missing);
        assert_eq!(slot(&details).version, version);
    }
}
