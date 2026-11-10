//! AMD GPU temperature through ADLX.
//!
//! The ADLX runtime (`amdadlx64.dll`) is installed by the graphics driver. It
//! is loaded at runtime and the few COM-like interfaces we need are described
//! with minimal vtables, so no AMD SDK is required at build time.
//!
//! The rung first asks ADLX for the GPU list, then for the current performance
//! metrics of each GPU and reads `IADLXGPUMetrics::GPUTemperature`.

use std::ffi::c_void;
use std::ptr;
use std::sync::OnceLock;

use super::dll::Dll;
use crate::provider::ProviderError;

type AdlxResult = i32;

const ADLX_OK: AdlxResult = 0;
const ADLX_ALREADY_ENABLED: AdlxResult = 1;
const ADLX_ALREADY_INITIALIZED: AdlxResult = 2;
const ADLX_BAD_VER: AdlxResult = 5;

/// Version advertised by the SDK headers used for this binding (2.0.0.125).
const ADLX_FULL_VERSION: u64 = (2u64 << 48) | 125;
/// Fallback for older runtimes that reject the SDK version.
const ADLX_MIN_VERSION: u64 = 1u64 << 48;

fn succeeded(code: AdlxResult) -> bool {
    matches!(
        code,
        ADLX_OK | ADLX_ALREADY_ENABLED | ADLX_ALREADY_INITIALIZED
    )
}

#[repr(C)]
struct IadlxSystem {
    vtbl: *const IadlxSystemVtbl,
}

#[repr(C)]
struct IadlxSystemVtbl {
    get_hybrid_graphics_type: *const c_void,
    get_gpus: unsafe extern "system" fn(*mut IadlxSystem, *mut *mut IadlxGpuList) -> AdlxResult,
    query_interface: *const c_void,
    get_displays_services: *const c_void,
    get_desktops_services: *const c_void,
    get_gpus_changed_handling: *const c_void,
    enable_log: *const c_void,
    get_3d_settings_services: *const c_void,
    get_gpu_tuning_services: *const c_void,
    get_performance_monitoring_services: unsafe extern "system" fn(
        *mut IadlxSystem,
        *mut *mut IadlxPerformanceMonitoringServices,
    ) -> AdlxResult,
    total_system_ram: *const c_void,
    get_i2c: *const c_void,
}

#[repr(C)]
struct IadlxPerformanceMonitoringServices {
    vtbl: *const IadlxPerformanceMonitoringServicesVtbl,
}

#[repr(C)]
struct IadlxPerformanceMonitoringServicesVtbl {
    acquire: unsafe extern "system" fn(*mut IadlxPerformanceMonitoringServices) -> i32,
    release: unsafe extern "system" fn(*mut IadlxPerformanceMonitoringServices) -> i32,
    query_interface: *const c_void,
    get_sampling_interval_range: *const c_void,
    set_sampling_interval: *const c_void,
    get_sampling_interval: *const c_void,
    get_max_performance_metrics_history_size_range: *const c_void,
    set_max_performance_metrics_history_size: *const c_void,
    get_max_performance_metrics_history_size: *const c_void,
    clear_performance_metrics_history: *const c_void,
    get_current_performance_metrics_history_size: *const c_void,
    start_performance_metrics_tracking: *const c_void,
    stop_performance_metrics_tracking: *const c_void,
    get_all_metrics_history: *const c_void,
    get_gpu_metrics_history: *const c_void,
    get_system_metrics_history: *const c_void,
    get_fps_history: *const c_void,
    get_current_all_metrics: *const c_void,
    get_current_gpu_metrics: unsafe extern "system" fn(
        *mut IadlxPerformanceMonitoringServices,
        *mut IadlxGpu,
        *mut *mut IadlxGpuMetrics,
    ) -> AdlxResult,
    get_current_system_metrics: *const c_void,
    get_current_fps: *const c_void,
    get_supported_gpu_metrics: *const c_void,
    get_supported_system_metrics: *const c_void,
}

#[repr(C)]
struct IadlxGpuList {
    vtbl: *const IadlxGpuListVtbl,
}

#[repr(C)]
struct IadlxGpuListVtbl {
    acquire: unsafe extern "system" fn(*mut IadlxGpuList) -> i32,
    release: unsafe extern "system" fn(*mut IadlxGpuList) -> i32,
    query_interface: *const c_void,
    size: unsafe extern "system" fn(*mut IadlxGpuList) -> u32,
    empty: *const c_void,
    begin: *const c_void,
    end: *const c_void,
    at: unsafe extern "system" fn(*mut IadlxGpuList, u32, *mut *mut IadlxGpu) -> AdlxResult,
    clear: *const c_void,
    remove_back: *const c_void,
    add_back: *const c_void,
}

#[repr(C)]
struct IadlxGpu {
    vtbl: *const IadlxGpuVtbl,
}

#[repr(C)]
struct IadlxGpuVtbl {
    acquire: unsafe extern "system" fn(*mut IadlxGpu) -> i32,
    release: unsafe extern "system" fn(*mut IadlxGpu) -> i32,
    query_interface: *const c_void,
    vendor_id: *const c_void,
    asic_family_type: *const c_void,
    gpu_type: *const c_void,
    is_external: *const c_void,
    name: *const c_void,
}

#[repr(C)]
struct IadlxGpuMetrics {
    vtbl: *const IadlxGpuMetricsVtbl,
}

#[repr(C)]
struct IadlxGpuMetricsVtbl {
    acquire: unsafe extern "system" fn(*mut IadlxGpuMetrics) -> i32,
    release: unsafe extern "system" fn(*mut IadlxGpuMetrics) -> i32,
    query_interface: *const c_void,
    timestamp: *const c_void,
    gpu_usage: *const c_void,
    gpu_clock_speed: *const c_void,
    gpu_vram_clock_speed: *const c_void,
    gpu_temperature: unsafe extern "system" fn(*mut IadlxGpuMetrics, *mut f64) -> AdlxResult,
}

type AdlxInitializeFn = unsafe extern "system" fn(u64, *mut *mut IadlxSystem) -> AdlxResult;
type AdlxTerminateFn = unsafe extern "system" fn() -> AdlxResult;
type AdlxQueryFullVersionFn = unsafe extern "system" fn(*mut u64) -> AdlxResult;

struct Adlx {
    _dll: Dll,
    terminate: AdlxTerminateFn,
    system: *mut IadlxSystem,
}

// ADLX documents its interfaces as callable from simultaneous threads. The
// process-wide instance is initialized once and only read afterwards.
unsafe impl Send for Adlx {}
unsafe impl Sync for Adlx {}

impl Adlx {
    fn read_temperature(&self) -> Result<f64, String> {
        unsafe {
            let system = &*self.system;
            let system_vtbl = &*system.vtbl;

            let mut list = ptr::null_mut();
            let code = (system_vtbl.get_gpus)(self.system, &mut list);
            if !succeeded(code) || list.is_null() {
                return Err(format!("ADLX GetGPUs failed with code {code}"));
            }
            let list_ref = &*list;
            let list_vtbl = &*list_ref.vtbl;
            let count = (list_vtbl.size)(list);
            if count == 0 {
                (list_vtbl.release)(list);
                return Err("ADLX reports no AMD GPU".to_string());
            }

            let mut services = ptr::null_mut();
            let code =
                (system_vtbl.get_performance_monitoring_services)(self.system, &mut services);
            if !succeeded(code) || services.is_null() {
                (list_vtbl.release)(list);
                return Err(format!(
                    "ADLX GetPerformanceMonitoringServices failed with code {code}"
                ));
            }
            let services_ref = &*services;
            let services_vtbl = &*services_ref.vtbl;

            let mut last_error = "no AMD GPU temperature".to_string();
            let mut result = None;
            for index in 0..count {
                let mut gpu = ptr::null_mut();
                let code = (list_vtbl.at)(list, index, &mut gpu);
                if !succeeded(code) || gpu.is_null() {
                    last_error = format!("ADLX GPU {index} could not be opened (code {code})");
                    continue;
                }
                let gpu_ref = &*gpu;
                let gpu_vtbl = &*gpu_ref.vtbl;

                let mut metrics = ptr::null_mut();
                let code = (services_vtbl.get_current_gpu_metrics)(services, gpu, &mut metrics);
                if succeeded(code) && !metrics.is_null() {
                    let metrics_ref = &*metrics;
                    let metrics_vtbl = &*metrics_ref.vtbl;
                    let mut celsius = 0.0f64;
                    let code = (metrics_vtbl.gpu_temperature)(metrics, &mut celsius);
                    if succeeded(code) && celsius.is_finite() {
                        result = Some(celsius);
                    } else {
                        last_error =
                            format!("ADLX GPU {index} temperature failed with code {code}");
                    }
                    (metrics_vtbl.release)(metrics);
                } else {
                    last_error = format!("ADLX GPU {index} metrics failed with code {code}");
                }

                (gpu_vtbl.release)(gpu);
                if result.is_some() {
                    break;
                }
            }

            (services_vtbl.release)(services);
            (list_vtbl.release)(list);
            result.ok_or(last_error)
        }
    }
}

impl Drop for Adlx {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.terminate)();
        }
    }
}

static ADLX: OnceLock<Result<Adlx, String>> = OnceLock::new();

fn init() -> Result<Adlx, String> {
    let dll = Dll::load("amdadlx64.dll")?;
    let initialize: AdlxInitializeFn = unsafe { dll.symbol("ADLXInitialize")? };
    let terminate: AdlxTerminateFn = unsafe { dll.symbol("ADLXTerminate")? };
    let query: Option<AdlxQueryFullVersionFn> = unsafe { dll.symbol("ADLXQueryFullVersion").ok() };

    let mut version = ADLX_FULL_VERSION;
    if let Some(query) = query {
        let mut runtime = 0u64;
        let queried = unsafe { query(&mut runtime) };
        if succeeded(queried) && runtime != 0 {
            version = runtime;
        }
    }

    let mut system = ptr::null_mut();
    let mut code = unsafe { initialize(version, &mut system) };
    if code == ADLX_BAD_VER {
        system = ptr::null_mut();
        code = unsafe { initialize(ADLX_MIN_VERSION, &mut system) };
    }
    if !succeeded(code) || system.is_null() {
        return Err(format!("ADLXInitialize failed with code {code}"));
    }

    Ok(Adlx {
        _dll: dll,
        terminate,
        system,
    })
}

/// Reads the first working AMD GPU temperature.
pub fn read() -> Result<f64, ProviderError> {
    let adlx = ADLX
        .get_or_init(init)
        .as_ref()
        .map_err(|error| ProviderError::Unavailable(error.clone()))?;
    adlx.read_temperature().map_err(ProviderError::Io)
}
