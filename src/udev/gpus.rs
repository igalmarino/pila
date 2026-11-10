//! The GPUs' renderers (`docs/MULTI_GPU.md`): Smithay's OpenGL ES on GBM
//! devices, with software renderers told apart by what draws, not by their
//! EGL device.
//!
//! A frame for another GPU is copied either through a dmabuf that GPU
//! imports or, where that can't work, through memory. A software renderer
//! can't import another device's buffer: Mesa's maps it as a dumb buffer,
//! which the kernel refuses for an imported one, and its shaders then read
//! from nowhere (a segfault). Smithay leaves software renderers out of such
//! imports, but knows them by `EGL_MESA_device_software`, which llvmpipe on
//! a KMS device without a GPU (`kms_swrast`: a virtual machine's display,
//! vkms) doesn't have. `GpuDevice` goes by the renderer's name too.

use std::{collections::HashMap, ffi::CStr, fmt};

use smithay::backend::{
    allocator::{dmabuf::AnyError, dmabuf::Dmabuf, gbm::GbmDevice, Allocator},
    drm::{DrmDeviceFd, DrmNode},
    egl::{EGLDisplay, Error as EglError},
    renderer::{
        gles::{ffi, GlesRenderer},
        multigpu::{
            gbm::{Error, GbmGlesBackend, GbmGlesDevice},
            ApiDevice, GraphicsApi,
        },
    },
};

/// Smithay's `GbmGlesBackend`, its devices wrapped in `GpuDevice`.
pub struct GpuApi(GbmGlesBackend<GlesRenderer, DrmDeviceFd>);

impl GpuApi {
    /// Renderers are made by `factory`, from their GPU's display.
    pub fn with_factory(
        factory: impl Fn(&EGLDisplay) -> Result<GlesRenderer, Error> + 'static,
    ) -> Self {
        Self(GbmGlesBackend::with_factory(factory))
    }

    pub fn add_node(&mut self, node: DrmNode, gbm: GbmDevice<DrmDeviceFd>) -> Result<(), EglError> {
        self.0.add_node(node, gbm)
    }

    pub fn remove_node(&mut self, node: &DrmNode) {
        self.0.remove_node(node);
    }
}

impl GraphicsApi for GpuApi {
    type Device = GpuDevice;
    type Error = Error;

    fn enumerate(&self, list: &mut Vec<GpuDevice>) -> Result<(), Error> {
        // Smithay's list, and back, whatever it says: devices it kept stay
        // as they were.
        let software: HashMap<DrmNode, bool> = list
            .iter()
            .map(|device| (*device.inner.node(), device.software))
            .collect();
        let mut inner: Vec<_> = list.drain(..).map(|device| device.inner).collect();
        let result = self.0.enumerate(&mut inner);
        list.extend(inner.into_iter().map(|mut inner| {
            let software = software
                .get(inner.node())
                .copied()
                .unwrap_or_else(|| draws_in_software(inner.renderer_mut()));
            GpuDevice { inner, software }
        }));
        result
    }

    fn needs_enumeration(&self) -> bool {
        self.0.needs_enumeration()
    }

    fn identifier() -> &'static str {
        "pila_gbm_gles"
    }
}

/// A GPU's renderer and allocator, and whether it draws in software.
pub struct GpuDevice {
    inner: GbmGlesDevice<GlesRenderer>,
    software: bool,
}

impl fmt::Debug for GpuDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuDevice")
            .field("inner", &self.inner)
            .field("software", &self.software)
            .finish()
    }
}

impl ApiDevice for GpuDevice {
    type Renderer = GlesRenderer;

    fn renderer(&self) -> &GlesRenderer {
        self.inner.renderer()
    }

    fn renderer_mut(&mut self) -> &mut GlesRenderer {
        self.inner.renderer_mut()
    }

    fn allocator(&mut self) -> &mut dyn Allocator<Buffer = Dmabuf, Error = AnyError> {
        self.inner.allocator()
    }

    fn node(&self) -> &DrmNode {
        self.inner.node()
    }

    // Neither way with a software renderer: frames go through memory.
    fn can_do_cross_device_imports(&self) -> bool {
        !self.software
    }

    fn should_do_cross_device_exports(&self) -> bool {
        !self.software
    }
}

/// Whether `renderer` draws on the CPU: as Smithay knows, or by its name.
fn draws_in_software(renderer: &mut GlesRenderer) -> bool {
    if renderer.is_software() {
        return true;
    }
    let name = renderer
        .with_context(|gl| {
            // SAFETY: the context is current in `with_context`; a string GL
            // returns is NUL-terminated and lives as long as the context.
            let name = unsafe { gl.GetString(ffi::RENDERER) };
            if name.is_null() {
                return String::new();
            }
            unsafe { CStr::from_ptr(name.cast()) }
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    let software = is_software_renderer(&name);
    if software {
        tracing::info!(
            name,
            "a software renderer: frames for another GPU go through memory"
        );
    }
    software
}

/// Whether a `GL_RENDERER` string names one of Mesa's software renderers.
fn is_software_renderer(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["llvmpipe", "softpipe", "swrast"]
        .iter()
        .any(|software| name.contains(software))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_renderers_are_known_by_name() {
        for name in [
            "llvmpipe (LLVM 20.1.2, 256 bits)",
            "softpipe",
            "Mesa swrast",
        ] {
            assert!(is_software_renderer(name), "{name}");
        }
        for name in [
            "AMD Radeon Graphics (radeonsi, renoir, ACO, DRM 3.64)",
            "Mesa Intel(R) UHD Graphics (CML GT2)",
            "NVIDIA GeForce RTX 3060/PCIe/SSE2",
            "",
        ] {
            assert!(!is_software_renderer(name), "{name}");
        }
    }
}
