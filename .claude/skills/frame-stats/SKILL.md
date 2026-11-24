---
name: frame-stats
description: What Pila's frame statistics log (RUST_LOG=pila::frames=debug, frame_stats.rs) and scripts/check report mean - idle, direct, torn, late, missed refreshes, GPU time, render-late latency, captures, copies, loop, uploads. Use when measuring a change to rendering, input or the event loop, or reading pila-check.log, pila-tty.log or a user's frame numbers.
---

# Frame statistics

The budget and the rules are in CLAUDE.md, "Performance pillar". This is how to read the numbers.

`RUST_LOG=pila::frames=debug` (`frame_stats.rs`) logs per output,
every 5 s, frames, average and worst CPU time and missed refreshes, plus every frame
over half the period, and apart the redraws that found nothing new to show (`idle`, with
their summed CPU time; `scripts/check` reports them per output) and, on bare metal, the
frames that were a client's buffer as it is (`direct`: direct scanout, nothing drawn; why
not, when it changes: `direct scanout refused` with the buffer's format and modifier, and
`dmabuf feedback` with `scanout` for a surface told the plane's formats) and
those shown at once by an asynchronous flip (`torn`: a fullscreen game with vsync off, which
miss no refresh).
Missed refreshes are vblanks with no new frame while an animation runs and, on bare metal,
a frame asked for in time but shown a vblank late, animation or not (`late`): exact on bare
metal (the DRM sequence of the vblanks that showed frames) and an estimate nested (gaps
over 1.5 periods; the host decides when frames show); `pila::frames=trace` logs every
frame. The first frame after an output is set up (connected, powered on, a new mode, back
from another VT: swapchain and modeset) is a cost of setting it up, logged on its own and
not counted. The GPU's part of a frame (`gpu_avg_ms`, `gpu_max_ms`: from its submission to
its native fence signaling, not waited for; none on software rendering) is logged with the
rest, and `scripts/check` reports it per output, unjudged: the budget is CPU time. On bare
metal frames are drawn late while the scene is steady, as close to their vblank as recent
frames allow (`render-late`, `udev/render_late.rs`), and their latency (redraw asked →
vblank that showed it) is logged and reported too; `PILA_CHECK_RENDER_LATE=false
scripts/check` gives the numbers without it. Screen captures, which build frames of their
own after the outputs', are logged apart (`captures`), held to the same budget
(`scripts/check` streams one, as a screen share does: `scripts/capture-stream`). The event
loop's own work after each iteration (`after_frames`: taskbars, IPC, the `Space`), paced
by events rather than frames, is logged apart too (`loop`), and so are clients' shm buffers,
copied to the GPU at their commits rather than in the frame (`uploads`, `upload.rs`), and
captures' copies into shm, their part on the event loop once the GPU is done with the frame
they read back (`copies`, `Pila::when_copied`: mapping the read-back, and writing a portal
share's buffer; a capture client's buffer and a screenshot are written on the copy thread,
`copy_thread.rs`, so a copy there is ~0.05 ms).
`scripts/check` holds each upload and each copy to 8 ms: half a 60 Hz period, render late's
margin, since they hold the event loop and any frame due meanwhile.
