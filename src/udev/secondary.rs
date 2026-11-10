//! Monitors on a GPU other than the primary (`docs/MULTI_GPU.md`): drawn on
//! the primary through a `MultiRenderer`, which copies each frame to a buffer
//! that GPU scans out.

use smithay::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::GlesRenderer,
        multigpu::Error as MultiError,
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        RendererSuper,
    },
    utils::{user_data::UserDataMap, Buffer as BufferCoords, Physical, Point, Rectangle, Scale},
};

use super::PrimaryRenderer;
use crate::render::PilaRenderElement;

/// One of Pila's elements, built on the primary's `GlesRenderer`, drawn on a
/// `MultiRenderer` frame: rendering is always on the primary, so its
/// `GlesFrame` (`as_mut`) is where the element's textures live. Never scanned
/// out directly: the frame is a copy on the other GPU.
pub(super) struct PrimaryGpuElement(pub PilaRenderElement);

impl Element for PrimaryGpuElement {
    fn id(&self) -> &Id {
        self.0.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.0.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.0.location(scale)
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.0.src()
    }

    fn transform(&self) -> smithay::utils::Transform {
        self.0.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.0.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.0.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.0.alpha()
    }

    fn kind(&self) -> Kind {
        self.0.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.0.is_framebuffer_effect()
    }
}

impl<'a> RenderElement<PrimaryRenderer<'a>> for PrimaryGpuElement {
    fn draw(
        &self,
        frame: &mut <PrimaryRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), <PrimaryRenderer<'a> as RendererSuper>::Error> {
        RenderElement::<GlesRenderer>::draw(
            &self.0,
            frame.as_mut(),
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
        .map_err(MultiError::Render)
    }

    fn underlying_storage(
        &self,
        _renderer: &mut PrimaryRenderer<'a>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn capture_framebuffer(
        &self,
        frame: &mut <PrimaryRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), <PrimaryRenderer<'a> as RendererSuper>::Error> {
        RenderElement::<GlesRenderer>::capture_framebuffer(&self.0, frame.as_mut(), src, dst, cache)
            .map_err(MultiError::Render)
    }
}

use super::{
    draw::frame_finish, outputs, scan_connectors, GpuDevice, OutputId, UdevData, COLOR_FORMATS,
};
use crate::{backend::Backend, state::Pila};
use smithay::{
    backend::{
        allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        drm::{
            exporter::gbm::GbmFramebufferExporter, output::DrmOutputManager, DrmDevice,
            DrmDeviceFd, DrmEvent, DrmNode, NodeType,
        },
        egl::{EGLDevice, EGLDisplay},
        session::Session,
    },
    reexports::rustix::fs::OFlags,
    utils::DeviceFd,
};
use smithay_drm_extras::drm_scanner::DrmScanner;
use std::path::Path;
use tracing::{info, warn};

/// Open another GPU of the seat and light up its monitors. A GPU already
/// open, the primary included, is left alone.
pub(super) fn add_gpu(state: &mut Pila, path: &Path) {
    if let Err(err) = try_add_gpu(state, path) {
        warn!(path = %path.display(), %err, "can't use a GPU: its monitors stay off");
    }
}

fn try_add_gpu(state: &mut Pila, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return Ok(());
    };
    let node = DrmNode::from_path(path)?;
    let node = node
        .node_with_type(NodeType::Primary)
        .and_then(Result::ok)
        .unwrap_or(node);
    if data.devices.contains_key(&node) {
        return Ok(());
    }
    let fd = data.session.open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;
    // Only to find the render node (see `init_udev`).
    // SAFETY: the GBM device outlives it (the managers below hold it).
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    let render_node = EGLDevice::device_for_display(&display)?
        .try_get_render_node()
        .ok()
        .flatten()
        .unwrap_or(node);
    // Its own renderer: what its outputs are set up with, and the copies
    // land through.
    data.gpus.as_mut().add_node(render_node, gbm.clone())?;
    let render_formats = {
        let mut renderer = data.gpus.single_renderer(&render_node)?;
        renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .clone()
    };
    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());
    let drm = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        COLOR_FORMATS,
        render_formats,
    );
    let events = data
        .loop_handle
        .insert_source(notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => frame_finish(state, OutputId { node, crtc }, metadata),
            DrmEvent::Error(err) => tracing::error!(?err, "DRM error"),
        })
        .map_err(|err| err.error)?;
    data.devices.insert(
        node,
        GpuDevice {
            drm,
            scanner: DrmScanner::new(),
            render_node,
            events: Some(events),
        },
    );
    info!(path = %path.display(), %render_node, "using another GPU for its monitors");
    scan_connectors(state, Some(node));
    state.apply_outputs();
    Ok(())
}

/// A GPU went away: its monitors as unplugged. The primary can't: it draws
/// everything.
pub(super) fn remove_gpu(state: &mut Pila, device_id: u64) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(node) = data
        .devices
        .keys()
        .copied()
        .find(|node| node.dev_id() == device_id)
    else {
        return;
    };
    if data.devices[&node].render_node == data.primary {
        warn!(%node, "the primary GPU went away: its monitors stay as they were");
        return;
    }
    let ids: Vec<OutputId> = data
        .outputs
        .keys()
        .copied()
        .filter(|id| id.node == node)
        .collect();
    for id in ids {
        outputs::connector_disconnected(state, id);
    }
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    remove_device(data, node);
    info!(%node, "a GPU went away");
    state.apply_outputs();
}

/// Drop a secondary GPU's device, its DRM events and its renderer.
fn remove_device(data: &mut UdevData, node: DrmNode) {
    let Some(device) = data.devices.remove(&node) else {
        return;
    };
    if let Some(events) = device.events {
        data.loop_handle.remove(events);
    }
    data.gpus.as_mut().remove_node(&device.render_node);
}
