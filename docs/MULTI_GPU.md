# Plan: more than one GPU

A plan being carried out (step 1 below is under way). It moves Pila from one GPU
(`udev/`: the seat's primary, which renders and scans out) to Smithay's `GpuManager` and
`MultiRenderer`, so monitors on a second GPU (a dock on a discrete card, a hybrid laptop's
HDMI port wired to the dGPU) light up. Read with `docs/ARCHITECTURE.md` ("Bare metal",
"Render loop", "Screen capture", "Blur", "Rounded corners", "Shadows").

## Where Pila is

- `GlesRenderer` is named 96 times in 12 files: `render.rs` (38), `rounded.rs` (14),
  `capture.rs` (11), `blur.rs` (9), `winit.rs` (5), `udev/mod.rs`, `headless.rs` and
  `backend.rs` (4 each), `decoration.rs` (3), `window_menu.rs` (2), `udev/outputs.rs` and
  `udev/draw.rs` (1 each).
- `PilaRenderElement` (`render.rs`) is `render_elements!` over `GlesRenderer`: client
  surfaces, solid colors, memory buffers (title bars, the window menu), `TextureRenderElement
  <GlesTexture>` (animation snapshots, blurred backdrops) and four elements of Pila's own.
- GL beyond Smithay's `Renderer` traits, in three places:
  - `rounded.rs`: custom shaders (`compile_custom_texture_shader`/`_pixel_shader`), drawn
    through `GlesFrame` (`render_texture_from_to` with a `GlesTexProgram`, `render_pixel_shader_to`)
    by `RoundedSurface`, `RoundedSolid`, `ShadowElement`.
  - `blur.rs`: its own shaders, offscreen `GlesTexture`s, and raw GL on them through
    `GlesRenderer::with_context` (wrap modes, filters).
  - `capture.rs`: the EGL context's dmabuf render formats, binding a client's dmabuf,
    `ExportMem` reads into shm, and on bare metal a blit of the frame the DRM compositor just
    composited (`blit_frame_result`, `GlesTarget`).
- `udev/mod.rs`: one `GbmDevice`, one `GlesRenderer` (`UdevData::renderer`), one
  `DrmOutputManager`, outputs keyed by CRTC; dmabuf feedback, the capture dmabuf target and
  linux-drm-syncobj all name that one device.
- `Backend::renderer()` hands out that one renderer for imports (dmabuf test-imports,
  captures, blur, animation snapshots).

## Where it goes

The model niri uses, and Smithay's multigpu module is built for: **render everything on the
primary GPU; other GPUs only scan out.** For an output on a secondary GPU,
`GpuManager::renderer(&primary, &target, format)` gives a `MultiRenderer` that renders on the
primary and copies the result to a buffer the target can scan out.

- `UdevData` holds a `GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>`, and per DRM device
  (`DrmNode`) its `DrmOutputManager`, GBM device and scanner. Outputs are keyed by
  `(DrmNode, crtc::Handle)`.
- Offscreen work (blur, animation snapshots, captures, `render::warm_up`) uses the primary's own
  `GlesRenderer` (`GpuManager::single_renderer(&primary)`, then `as_mut()`), lent through
  `Backend::renderer`'s `RendererRef`. Its textures are `GlesTexture`s on the primary.
- Drawing an output on the primary: as now, on that `GlesRenderer`. On a secondary: a
  `MultiRenderer` for `(primary, output's device)`, which renders on the primary and copies the
  result; Pila's elements go through one wrapper (below).
- Nested and headless keep a plain `GlesRenderer` (one GPU, or none).

## One wrapper element, not generic code

The render GPU is always the primary, so the elements Pila builds today on the primary's
`GlesRenderer` can be drawn as they are on a `MultiRenderer` frame: its `GlesFrame` is
`frame.as_mut()` (Smithay implements `AsMut` for `MultiFrame` and `MultiRenderer`). One element,
`PrimaryGpuElement` (`udev/`), wraps a `PilaRenderElement` and implements
`RenderElement<MultiRenderer>` by drawing it on that `GlesFrame` (errors become
`MultiError::Render`); `underlying_storage` is `None`, so nothing on a secondary output is
scanned out directly. `render.rs`, `rounded.rs`, `blur.rs`, `capture.rs`, `decoration.rs` and
`window_menu.rs` stay on `GlesRenderer`, as do nested and headless. (niri makes its element
code generic over a `NiriRenderer` trait instead, about a hundred changes in a dozen files
here, for the same result while rendering stays on one GPU.)

The cost: the primary's `GlesRenderer` lives in the `GpuManager` (its textures, client
imports included, must belong to the context its frames draw with), and `GpuManager` lends it
only through a `MultiRenderer` borrowed from itself (`single_renderer(&primary)`, then
`as_mut()`). So `Backend::renderer()` returns a guard, `RendererRef`, that derefs to the
`GlesRenderer` and holds that loan on bare metal (`None` if the GPU manager can't lend it);
niri lends it to a closure instead (`with_primary_renderer`).

## In what order, without breaking anything

Each step keeps the suite green and `scripts/check --nested` at the same numbers.

1. **`GpuManager` with one GPU.** `udev/` creates a `GpuManager<GbmGlesBackend<GlesRenderer,
   DrmDeviceFd>>` with the primary in it, and takes its renderer from there (`RendererRef`
   outside the backend); outputs draw exactly as now. Bare metal must be
   unchanged: `scripts/check` on a TTY (and `vkms.yml`) before going on.
2. **Secondary GPUs.** Open every GPU of the seat, a `DrmOutputManager` and scanner for each;
   connectors of every device become outputs, keyed by `(DrmNode, crtc::Handle)`; `recovery.rs`,
   gamma, output power and the vblank handler (`frame_finish`) take the node.
   `UdevEvent::Added/Removed` for devices plug them in and out (their outputs as monitors
   unplugged). VT switches pause and resume every device. An output on a secondary device draws
   with `GpuManager::renderer(&primary, &its device, format)` and `PrimaryGpuElement`s; the
   copy is logged apart in the frame statistics. Tried in CI with two vkms devices
   (`vkms.yml`): device handling and outputs on the second one, not real copies between GPUs.
3. **What clients are told.** Per-surface dmabuf feedback (Smithay's `DmabufFeedbackBuilder`
   with a scanout tranche for the output's device), so a fullscreen client on a secondary output
   can allocate buffers that device scans out; linux-drm-syncobj stays on the primary.
   Captures stay on the primary; the composited-frame blit (`take_composited`) only applies to
   outputs on it, the others' captures are drawn from the scene.

## Risks

- **Frame budget.** A secondary output's frame is rendered on the primary and copied (a dmabuf
  import on the target, or a CPU copy if the formats don't meet). That's GPU time and, for a CPU
  copy, CPU time inside the 4 ms; the frame statistics will show it (log the copy on its own,
  like captures).
- **Formats and modifiers.** The copy needs a format both devices share; with none in common
  Smithay falls back to a CPU copy, which a 4K monitor can't afford at 60 Hz. Log the path taken.
- **Explicit sync across devices.** Fences for the copy are Smithay's; Pila's
  `wait_for_client_rendering` (implicit fences, syncobj points) is for the primary only.
- **Drivers.** NVIDIA's proprietary driver's GBM, and a dGPU that sleeps (runtime PM: an output on
  it wakes it). Hybrid laptops are the common case; the user's (AMD Renoir) has one GPU.
- **Pacing.** Each output already paces itself by its own vblanks; a secondary's vblank also
  waits for the copy. `idle_wait` and `late_refreshes` are per output and carry over.

## What can't be checked here

- Nothing with a second GPU: the container has no seat, and its headless backend has one
  renderer. Step 1 needs `scripts/check` on a TTY (one GPU); steps 2 and 3 need a machine with
  two GPUs and monitors on both, which the user doesn't have: they need a tester.
- Possibly in CI: `vkms.yml` runs the udev backend on the kernel's virtual KMS driver; kernels
  with vkms' configfs can make several vkms devices, which would exercise step 2's device handling
  and outputs on a second device (rendering on llvmpipe, scanning out on vkms), not real copies
  between GPUs.
