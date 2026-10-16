//! The Rendering GPU adapter probe (`AppEnv.list_adapters`, R-171, R-237).
//!
//! GPUI exposes no adapter list, so tl-app asks wgpu directly (R-142). The instance is built the
//! same way gpui-pre-wgpu 0.3.7 builds it (`WgpuContext::instance`: Vulkan and GL, default flags,
//! no display handle), so the list holds the adapters GPUI chooses from.
//!
//! GPUI picks an adapter only through `ZED_DEVICE_ID`, which `rendering_env` sets from a
//! `pci:<bus id>` value. An adapter without a PCI bus id (GL adapters, lavapipe) cannot be
//! picked that way, so the list leaves it out. "Software" covers the software adapters.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use tl_ui::app::AdapterInfo;
use tracing::{info, warn};

/// Starts the probe on its own thread and returns at once. The thread sends the list once.
pub fn list_adapters() -> async_channel::Receiver<Vec<AdapterInfo>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = thread::Builder::new()
        .name("tl-adapter-probe".into())
        .spawn(move || {
            let _ = tx.send_blocking(probe());
        });
    if let Err(e) = spawned {
        warn!(target: "tasklens::ui", "adapter probe thread did not start: {e}");
    }
    rx
}

/// The adapters wgpu finds that `rendering.gpu` can select, in wgpu's order.
pub fn probe() -> Vec<AdapterInfo> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
        flags: wgpu::InstanceFlags::default(),
        backend_options: wgpu::BackendOptions::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    });
    let adapters = block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let list = selectable(adapters.iter().map(|a| to_info(&a.get_info())));
    info!(target: "tasklens::ui", count = list.len(), "adapter probe done");
    list
}

/// Maps one wgpu adapter. An empty PCI bus id means the backend does not report one.
fn to_info(info: &wgpu::AdapterInfo) -> AdapterInfo {
    AdapterInfo {
        name: info.name.clone(),
        backend: format!("{:?}", info.backend),
        device_type: format!("{:?}", info.device_type),
        pci_bus_id: (!info.device_pci_bus_id.is_empty()).then(|| info.device_pci_bus_id.clone()),
    }
}

/// The adapters with a PCI bus id, the only ones a `pci:` value can select.
fn selectable(adapters: impl Iterator<Item = AdapterInfo>) -> Vec<AdapterInfo> {
    adapters.filter(|a| a.pci_bus_id.is_some()).collect()
}

/// Wakes the probe thread.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` on this thread. wgpu's native `enumerate_adapters` future is ready on the
/// first poll, so this does not loop in practice.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wgpu_info(bus: &str) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: "NVIDIA GeForce RTX 4070".to_owned(),
            vendor: 0x10de,
            device: 0x2786,
            device_type: wgpu::DeviceType::DiscreteGpu,
            device_pci_bus_id: bus.to_owned(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 32,
            subgroup_max_size: 32,
            transient_saves_memory: false,
        }
    }

    #[test]
    fn maps_bus_id() {
        assert_eq!(
            to_info(&wgpu_info("0000:01:00.0")),
            AdapterInfo {
                name: "NVIDIA GeForce RTX 4070".to_owned(),
                backend: "Vulkan".to_owned(),
                device_type: "DiscreteGpu".to_owned(),
                pci_bus_id: Some("0000:01:00.0".to_owned()),
            }
        );
    }

    #[test]
    fn empty_bus_id_is_none() {
        assert_eq!(to_info(&wgpu_info("")).pci_bus_id, None);
    }

    #[test]
    fn adapters_without_bus_id_left_out() {
        let gl = wgpu::AdapterInfo {
            backend: wgpu::Backend::Gl,
            ..wgpu_info("")
        };
        let list = selectable([to_info(&wgpu_info("0000:01:00.0")), to_info(&gl)].into_iter());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].pci_bus_id.as_deref(), Some("0000:01:00.0"));
    }

    #[test]
    fn block_on_waits_for_wake() {
        let (tx, rx) = async_channel::bounded(1);
        let sender = thread::spawn(move || tx.send_blocking(7_u32));
        assert_eq!(block_on(rx.recv()), Ok(7));
        assert!(sender.join().is_ok());
    }

    #[test]
    #[ignore = "real system: needs a GPU; run with TL_GPU_TESTS=1 and --ignored"]
    fn probe_finds_an_adapter() {
        if std::env::var_os("TL_GPU_TESTS").is_none_or(|v| v != "1") {
            return;
        }
        let adapters = block_on(list_adapters().recv()).unwrap_or_default();
        assert!(!adapters.is_empty(), "no adapter found");
        assert!(adapters.iter().all(|a| {
            a.pci_bus_id
                .as_deref()
                .and_then(crate::rendering_env::pci_device_id)
                .is_some()
        }));
    }
}
