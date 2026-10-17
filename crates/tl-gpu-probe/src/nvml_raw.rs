//! The two raw NVML calls that nvml-wrapper 0.13.0 has no safe method for: JPEG and OFA engine
//! utilisation (R-292, PERF-063).
//!
//! Both bindings come from nvml-wrapper-sys 0.10.0 through `Nvml::lib()`. A driver whose NVML has
//! no such symbol gives `NotSupported`, the same as a device without the engine, so the engine
//! leaves the row out. This runs in the probe process, so a driver fault in these calls costs only
//! the probe.
//!
//! This is one of the five files allowed to hold `unsafe` (ADR-001 rule 4).
#![allow(unsafe_code)]

use std::ffi::c_uint;

use nvml_wrapper::Device;
use nvml_wrapper::error::{NvmlError, nvml_try};

/// `nvmlDeviceGetJpgUtilization`, percent of the sampling period the JPEG engine was busy.
pub fn jpg_utilization(dev: &Device<'_>) -> Result<u32, NvmlError> {
    let Ok(sym) = dev.nvml().lib().nvmlDeviceGetJpgUtilization.as_ref() else {
        return Err(NvmlError::NotSupported);
    };
    let mut util: c_uint = 0;
    let mut period_us: c_uint = 0;
    // SAFETY: `dev` borrows the live `Nvml` that loaded `sym`, so `dev.handle()` is a valid
    // device handle for the whole call. Both out pointers point to local `c_uint`s that outlive
    // the call, and NVML writes only one `unsigned int` to each.
    let rc = unsafe { sym(dev.handle(), &mut util, &mut period_us) };
    nvml_try(rc)?;
    Ok(util)
}

/// `nvmlDeviceGetOfaUtilization`, percent of the sampling period the optical flow engine was
/// busy.
pub fn ofa_utilization(dev: &Device<'_>) -> Result<u32, NvmlError> {
    let Ok(sym) = dev.nvml().lib().nvmlDeviceGetOfaUtilization.as_ref() else {
        return Err(NvmlError::NotSupported);
    };
    let mut util: c_uint = 0;
    let mut period_us: c_uint = 0;
    // SAFETY: as in `jpg_utilization`: a valid handle from the live `Nvml` that loaded `sym`,
    // and two local out pointers that outlive the call.
    let rc = unsafe { sym(dev.handle(), &mut util, &mut period_us) };
    nvml_try(rc)?;
    Ok(util)
}
