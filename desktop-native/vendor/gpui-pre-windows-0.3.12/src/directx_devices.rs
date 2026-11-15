use anyhow::{Context, Result};
use gpui_util::ResultExt;
use itertools::Itertools;
use windows::Win32::{
    Foundation::HMODULE,
    Graphics::{
        Direct3D::{
            D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
        },
        Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_DEBUG,
            D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS, D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS,
            D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
        },
        Dxgi::{
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS,
            IDXGIAdapter1, IDXGIFactory6,
        },
    },
};
use windows::core::Interface;

pub(crate) fn try_to_recover_from_device_lost<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    (0..5)
        .map(|i| {
            if i > 0 {
                // Add a small delay before retrying
                std::thread::sleep(std::time::Duration::from_millis(100 + i * 10));
            }
            f()
        })
        .find_or_last(Result::is_ok)
        .unwrap()
        .context("DirectXRenderer failed to recover from lost device after multiple attempts")
}

/// Hangar: the device GPUI draws with, for textures painted with `Window::paint_surface`. Replaced on device loss.
static SHARED_DEVICE: std::sync::Mutex<Option<SharedDevice>> = std::sync::Mutex::new(None);

#[derive(Clone)]
struct SharedDevice(ID3D11Device);
// SAFETY: `ID3D11Device` is free-threaded; only resource creation goes through it here.
unsafe impl Send for SharedDevice {}
unsafe impl Sync for SharedDevice {}

fn shared_device() -> Option<ID3D11Device> {
    SHARED_DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map(|d| d.0.clone())
}

/// Hangar: an external texture for `Window::paint_surface`, straight-alpha RGBA on the device GPUI draws with. One made
/// on a device that was since lost is skipped by the renderer, and `is_current` tells the owner to make a new one.
pub struct SurfaceTexture {
    pub(crate) view: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
    texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    pub(crate) device: ID3D11Device,
    pub(crate) width: u32,
    pub(crate) height: u32,
}
// SAFETY: the texture is only written through `write`, on the thread that draws, like every other use of the context.
unsafe impl Send for SurfaceTexture {}
unsafe impl Sync for SurfaceTexture {}

impl SurfaceTexture {
    /// An RGBA texture of this size on the device GPUI draws with, filled with `write`.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};
        anyhow::ensure!(width > 0 && height > 0, "empty surface");
        let device = shared_device().context("no DirectX device yet")?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: plain descriptor, no initial data.
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.context("creating surface texture")?;
        let texture = texture.context("surface texture missing")?;
        let mut view = None;
        // SAFETY: `texture` was just created on `device` with a shader-resource binding.
        unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut view)) }.context("creating surface view")?;
        Ok(Self { view: view.context("surface view missing")?, texture, device, width, height })
    }

    /// Replaces the contents with `pixels` (RGBA, `width * 4` bytes per row). Only on the thread that draws the windows:
    /// it goes through the device's immediate context, which the renderer also uses.
    pub fn write(&self, pixels: &[u8]) -> Result<()> {
        let needed = (self.width as usize).checked_mul(self.height as usize).and_then(|n| n.checked_mul(4)).context("surface too large")?;
        anyhow::ensure!(pixels.len() >= needed, "surface pixels too short");
        // SAFETY: the immediate context of the device that owns the texture.
        let context = unsafe { self.device.GetImmediateContext() }.context("no immediate context")?;
        // SAFETY: `pixels` holds `height` rows of `width * 4` bytes and outlives the call.
        unsafe { context.UpdateSubresource(&self.texture, 0, None, pixels.as_ptr().cast(), self.width * 4, 0) };
        Ok(())
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }

    /// Made on the device GPUI draws with now (false after a device loss).
    pub fn is_current(&self) -> bool { shared_device().is_some_and(|d| d == self.device) }
}

#[derive(Clone)]
pub(crate) struct DirectXDevices {
    pub(crate) adapter: IDXGIAdapter1,
    pub(crate) dxgi_factory: IDXGIFactory6,
    pub(crate) device: ID3D11Device,
    pub(crate) device_context: ID3D11DeviceContext,
}

impl DirectXDevices {
    pub(crate) fn new() -> Result<Self> {
        let debug_layer_available = check_debug_layer_available();
        let dxgi_factory =
            get_dxgi_factory(debug_layer_available).context("Creating DXGI factory")?;
        let (adapter, device, device_context, feature_level) =
            get_adapter(&dxgi_factory, debug_layer_available).context("Getting DXGI adapter")?;
        match feature_level {
            D3D_FEATURE_LEVEL_11_1 => {
                log::info!("Created device with Direct3D 11.1 feature level.")
            }
            D3D_FEATURE_LEVEL_11_0 => {
                log::info!("Created device with Direct3D 11.0 feature level.")
            }
            D3D_FEATURE_LEVEL_10_1 => {
                log::info!("Created device with Direct3D 10.1 feature level.")
            }
            _ => unreachable!(),
        }

        *SHARED_DEVICE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SharedDevice(device.clone()));
        Ok(Self {
            adapter,
            dxgi_factory,
            device,
            device_context,
        })
    }
}

#[inline]
fn check_debug_layer_available() -> bool {
    #[cfg(debug_assertions)]
    {
        use windows::Win32::Graphics::Dxgi::{DXGIGetDebugInterface1, IDXGIInfoQueue};

        unsafe { DXGIGetDebugInterface1::<IDXGIInfoQueue>(0) }
            .log_err()
            .is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

#[inline]
fn get_dxgi_factory(debug_layer_available: bool) -> Result<IDXGIFactory6> {
    let factory_flag = if debug_layer_available {
        DXGI_CREATE_FACTORY_DEBUG
    } else {
        #[cfg(debug_assertions)]
        log::warn!(
            "Failed to get DXGI debug interface. DirectX debugging features will be disabled."
        );
        DXGI_CREATE_FACTORY_FLAGS::default()
    };
    unsafe { Ok(CreateDXGIFactory2(factory_flag)?) }
}

#[inline]
fn get_adapter(
    dxgi_factory: &IDXGIFactory6,
    debug_layer_available: bool,
) -> Result<(
    IDXGIAdapter1,
    ID3D11Device,
    ID3D11DeviceContext,
    D3D_FEATURE_LEVEL,
)> {
    for adapter_index in 0.. {
        let adapter: IDXGIAdapter1 = unsafe { dxgi_factory.EnumAdapters(adapter_index)?.cast()? };
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            let gpu_name = String::from_utf16_lossy(&desc.Description)
                .trim_matches(char::from(0))
                .to_string();
            log::info!("Using GPU: {}", gpu_name);
        }
        // Check to see whether the adapter supports Direct3D 11 and create
        // the device if it does.
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        if let Some(device) = get_device(
            &adapter,
            Some(&mut context),
            Some(&mut feature_level),
            debug_layer_available,
        )
        .log_err()
        {
            return Ok((adapter, device, context.unwrap(), feature_level));
        }
    }

    unreachable!()
}

#[inline]
fn get_device(
    adapter: &IDXGIAdapter1,
    context: Option<*mut Option<ID3D11DeviceContext>>,
    feature_level: Option<*mut D3D_FEATURE_LEVEL>,
    debug_layer_available: bool,
) -> Result<ID3D11Device> {
    let mut device: Option<ID3D11Device> = None;
    let device_flags = if debug_layer_available {
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_DEBUG
    } else {
        D3D11_CREATE_DEVICE_BGRA_SUPPORT
    };
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            device_flags,
            // 4x MSAA is required for Direct3D Feature Level 10.1 or better
            Some(&[
                D3D_FEATURE_LEVEL_11_1,
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_1,
            ]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            feature_level,
            context,
        )?;
    }
    let device = device.unwrap();
    let mut data = D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS::default();
    unsafe {
        device
            .CheckFeatureSupport(
                D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS,
                &mut data as *mut _ as _,
                std::mem::size_of::<D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS>() as u32,
            )
            .context("Checking GPU device feature support")?;
    }
    if data
        .ComputeShaders_Plus_RawAndStructuredBuffers_Via_Shader_4_x
        .as_bool()
    {
        Ok(device)
    } else {
        Err(anyhow::anyhow!(
            "Required feature StructuredBuffer is not supported by GPU/driver"
        ))
    }
}
