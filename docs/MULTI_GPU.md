# Plan: more than one GPU

A plan, not a design that's been tried: no code follows it yet. It moves Pila from one GPU
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
  `GlesRenderer` (`GpuManager::single_renderer(&primary)`); `Backend::renderer()` returns it.
  Its textures are `GlesTexture`s on the primary.
- Drawing an output: `MultiRenderer` for `(primary, output's device)`. Its `TextureId` is
  `MultiTexture`, not `GlesTexture`, so elements that hold primary textures can't be
  `TextureRenderElement<GlesTexture>` as they are; they become a wrapper (as niri's
  `PrimaryGpuTextureRenderElement`) that draws them on the `GlesFrame` under the `MultiFrame`
  (`AsMut<GlesFrame>`, which Smithay implements). That's sound because the render GPU is always
  the primary.
- Nested and headless keep a plain `GlesRenderer` (one GPU, or none).

## What becomes generic, and what stays GL

- **Generic over the renderer** (`R: PilaRenderer`, a trait of the bounds Pila uses: `Renderer`
  + `ImportAll` + `ImportMem` + `ExportMem` + `Bind<Dmabuf>` + `Offscreen<GlesTexture>` +
  `AsMut<GlesRenderer>`, with `R::Frame: AsMut<GlesFrame>`; niri's `NiriRenderer` is this
  trait): `render.rs`'s element building (`output_elements`, `scene_elements`,
  `pointer_elements`, `window_elements`, layer and slide elements), `decoration.rs` and
  `window_menu.rs` (memory buffers), `frame.rs` (already renderer-free), the damage tracker's
  use in each backend. `PilaRenderElement<R>` becomes generic, as niri's `OutputRenderElements<R>`.
- **GL, kept GL, behind `AsMut`:** `rounded.rs`'s and `blur.rs`'s shaders and their raw GL
  (`with_context`) need `GlesRenderer`/`GlesFrame`; their `RenderElement<R>` impls take the GL
  one with `renderer.as_mut()` / `frame.as_mut()`. Shaders are compiled once, on the primary
  (`rounded::Shaders::get`, `blur::Shaders::get`), as today. Their `RenderElement` impl is the one place that
  knows of `MultiFrame`; nothing above them changes.
- **Captures** render on the primary's `GlesRenderer`, as offscreen work. Their dmabuf target is
  the primary's render node (it's what can render into it). The composited-frame blit
  (`take_composited`) only applies to outputs whose frame is on the primary; an output on a
  secondary GPU has its capture drawn from the scene instead (`process_captures`' path).

## In what order, without breaking anything

Each step keeps the suite green, `scripts/check --nested` at the same numbers, and bare metal
identical on one GPU; only the last two change behavior.

1. **The trait, with one implementor.** Add `PilaRenderer` and make `render.rs`, `decoration.rs`,
   `window_menu.rs` and the custom elements generic over it, implemented for `GlesRenderer`
   only. Texture elements go through the primary-texture wrapper. Mechanical; the compiler finds
   every place. No behavior change.
2. **The backends ask for it.** `Backend::renderer()` returns `&mut impl PilaRenderer`; winit and
   headless pass their `GlesRenderer`. Still one type.
3. **`GpuManager` with one GPU.** `udev/` creates a `GpuManager`, adds the primary, and draws each
   output with `MultiRenderer` for `(primary, primary)`. Outputs keyed by `(DrmNode, crtc)`;
   `recovery.rs`, gamma (`gamma.rs`), output power and the vblank handler (`frame_finish`) take the
   node. Bare metal must be unchanged: frame times and the first-frame cost checked with
   `scripts/check` on a TTY before going on.
4. **Secondary GPUs.** Open every GPU of the seat (`all_gpus`), a `DrmOutputManager` for each;
   connectors of every device become outputs; `UdevEvent::Added/Removed` for devices plug them in
   and out (their outputs as monitors unplugged). VT switches pause and resume every device.
5. **What clients are told.** Per-surface dmabuf feedback (Smithay's `DmabufFeedbackBuilder`
   with a scanout tranche for the output's device), so a fullscreen client on a secondary output
   can allocate buffers that device scans out; linux-drm-syncobj stays on the primary.

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
- **Bigger types.** `PilaRenderElement<R>` and generic element code compile once per renderer
  (two on bare metal builds): build time grows.

## What can't be checked here

- Nothing with a second GPU: the container has no seat, and its headless backend has one
  renderer. Steps 1 and 2 are fully covered by the suite and `scripts/check --nested`; step 3
  needs `scripts/check` on a TTY (one GPU); steps 4 and 5 need a machine with two GPUs and monitors
  on both, which the user doesn't have: they need a tester.
- Possibly in CI: `vkms.yml` runs the udev backend on the kernel's virtual KMS driver; kernels
  with vkms' configfs can make several vkms devices, which would exercise step 4's device handling
  and outputs on a second device (rendering on llvmpipe, scanning out on vkms), not real copies
  between GPUs.
