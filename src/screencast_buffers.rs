//! The buffers of a portal share's stream (`screencast.rs`), allocated by
//! Pila: dma-bufs from GBM when the app takes them (frames are drawn
//! straight into them: no copy through the CPU), else shared memory (a
//! memfd) that frames are read back into.
//!
//! Which one is negotiated with the app: the stream offers a format with the
//! modifiers Pila can draw into (`DmabufTarget`), left for it to pick from
//! (`DONT_FIXATE`), and one without, for shared memory. If the app's format
//! comes back with several modifiers, Pila fixes one by allocating a buffer
//! with them (GBM picks) and offers that alone (`Allocator::pick_modifier`).

use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
};

use pipewire::spa;
use smithay::backend::{
    allocator::{
        dmabuf::{AsDmabuf, Dmabuf},
        gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        Allocator as _, Buffer as _, Fourcc, Modifier,
    },
    drm::{DrmNode, NodeType},
};
use tracing::{debug, warn};

/// The format frames are drawn in: BGRx in memory.
pub const FOURCC: Fourcc = Fourcc::Xrgb8888;

/// GBM on the renderer's device, and the modifiers it can draw into.
pub struct Allocator {
    gbm: GbmAllocator<OwnedFd>,
    pub modifiers: Vec<u64>,
}

impl Allocator {
    /// An allocator on the render node of `device` (the renderer's), for
    /// `modifiers`; `None` if there's none to offer or the node can't be
    /// opened (shared memory then).
    pub fn open(device: u64, modifiers: Vec<u64>) -> Option<Self> {
        if modifiers.is_empty() {
            return None;
        }
        let node = DrmNode::from_dev_id(device).ok()?;
        let path = node
            .dev_path_with_type(NodeType::Render)
            .or_else(|| node.dev_path())?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|err| debug!(?err, ?path, "portal: can't open the render node"))
            .ok()?;
        let gbm = GbmDevice::new(OwnedFd::from(file))
            .map_err(|err| debug!(?err, "portal: no GBM device"))
            .ok()?;
        Some(Self {
            gbm: GbmAllocator::new(gbm, GbmBufferFlags::RENDERING),
            modifiers,
        })
    }

    /// The modifier of a `width` by `height` buffer allocated with one of
    /// `candidates` that Pila can draw into, and its planes: GBM picks.
    pub fn pick_modifier(
        &mut self,
        (width, height): (u32, u32),
        candidates: &[u64],
    ) -> Option<(u64, usize)> {
        let usable: Vec<Modifier> = candidates
            .iter()
            .filter(|m| self.modifiers.contains(m))
            .map(|&m| Modifier::from(m))
            .collect();
        let dmabuf = self.dmabuf((width, height), &usable)?;
        Some((u64::from(dmabuf.format().modifier), dmabuf.num_planes()))
    }

    /// A `width` by `height` dma-buf with one of `modifiers`.
    pub fn dmabuf(
        &mut self,
        (width, height): (u32, u32),
        modifiers: &[Modifier],
    ) -> Option<Dmabuf> {
        if modifiers.is_empty() {
            return None;
        }
        let buffer = self
            .gbm
            .create_buffer(width, height, FOURCC, modifiers)
            .map_err(|err| debug!(?err, "portal: can't allocate a dma-buf"))
            .ok()?;
        buffer
            .export()
            .map_err(|err| debug!(?err, "portal: can't export a dma-buf"))
            .ok()
    }
}

/// What a stream's buffers are, once negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Shared memory: frames are read back into it.
    Shm,
    /// Dma-bufs with this modifier and number of planes: frames are drawn
    /// into them.
    Dmabuf { modifier: u64, planes: usize },
}

/// A buffer's memory, kept as long as PipeWire has the buffer.
pub enum Memory {
    Dmabuf(Dmabuf),
    Shm(Shm),
}

/// A memfd mapped for writing.
pub struct Shm {
    _fd: OwnedFd,
    ptr: *mut u8,
    len: usize,
}

impl Drop for Shm {
    fn drop(&mut self) {
        // SAFETY: mapped by `Shm::new` with this length, and not used after.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

impl Shm {
    /// `len` bytes of shared memory, mapped, sealed at that size: the app
    /// sharing gets the fd, and shrinking it under the mapping Pila writes
    /// into would end Pila with SIGBUS.
    fn new(len: usize) -> Option<(Self, i64)> {
        // SAFETY: a plain syscall with a valid name.
        let fd = unsafe {
            libc::memfd_create(
                c"pila-screencast".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: a new fd of ours.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: plain syscalls on that fd.
        let ptr = unsafe {
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
                return None;
            }
            let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
            if libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) != 0 {
                return None;
            }
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return None;
        }
        let raw = i64::from(fd.as_raw_fd());
        Some((
            Self {
                _fd: fd,
                ptr: ptr.cast(),
                len,
            },
            raw,
        ))
    }
}

/// A stream's buffers by their PipeWire buffer, and what they are.
#[derive(Default)]
pub struct Buffers {
    pub kind: Option<Kind>,
    /// The size negotiated, which buffers are allocated at.
    pub size: (u32, u32),
    /// What the stream offers now: its frames' size and rate (the output's
    /// mode), which a modifier is fixed for.
    pub offered: ((i32, i32), u32),
    memory: HashMap<usize, Memory>,
}

impl Buffers {
    /// The memory of a dequeued `buffer`.
    pub fn get(&self, buffer: *mut pipewire::sys::pw_buffer) -> Option<&Memory> {
        self.memory.get(&(buffer as usize))
    }

    /// Allocate `buffer`'s memory, as negotiated, and describe it to
    /// PipeWire (`add_buffer`).
    ///
    /// # Safety
    ///
    /// `buffer` is a buffer PipeWire is adding to the stream, with as many
    /// datas as the negotiated `Buffers` param's blocks.
    pub unsafe fn add(
        &mut self,
        buffer: *mut pipewire::sys::pw_buffer,
        allocator: Option<&mut Allocator>,
    ) {
        let (width, height) = self.size;
        let stride = width * 4;
        // SAFETY: per this function's contract.
        let spa_buffer = unsafe { (*buffer).buffer };
        // A slice is never made of a null pointer, even an empty one.
        // SAFETY: `spa_buffer` is checked non-null first; PipeWire's.
        if spa_buffer.is_null() || unsafe { (*spa_buffer).datas.is_null() } {
            return;
        }
        // SAFETY: as above; `datas` has `n_datas` entries.
        let datas = unsafe {
            std::slice::from_raw_parts_mut((*spa_buffer).datas, (*spa_buffer).n_datas as usize)
        };
        let memory = match (self.kind, allocator) {
            (Some(Kind::Dmabuf { modifier, planes }), Some(allocator)) => {
                let Some(dmabuf) = allocator.dmabuf((width, height), &[Modifier::from(modifier)])
                else {
                    warn!("portal: can't allocate a buffer for a share");
                    return;
                };
                if dmabuf.num_planes() != planes || datas.len() < planes {
                    warn!(planes, "portal: a dma-buf of another shape than negotiated");
                    return;
                }
                for (((data, fd), offset), stride) in datas
                    .iter_mut()
                    .zip(dmabuf.handles())
                    .zip(dmabuf.offsets())
                    .zip(dmabuf.strides())
                {
                    data.type_ = spa::sys::SPA_DATA_DmaBuf;
                    data.flags = spa::sys::SPA_DATA_FLAG_READWRITE;
                    data.fd = i64::from(fd.as_fd().as_raw_fd());
                    data.mapoffset = 0;
                    data.maxsize = stride * height + offset;
                    data.data = std::ptr::null_mut();
                    // SAFETY: PipeWire gives each data its chunk.
                    if let Some(chunk) = unsafe { data.chunk.as_mut() } {
                        chunk.offset = offset;
                        chunk.stride = stride as i32;
                        chunk.size = stride * height;
                        chunk.flags = 0;
                    }
                }
                Memory::Dmabuf(dmabuf)
            }
            _ => {
                let len = (stride * height) as usize;
                let Some((shm, fd)) = Shm::new(len) else {
                    warn!("portal: can't allocate shared memory for a share");
                    return;
                };
                let Some(data) = datas.first_mut() else {
                    return;
                };
                data.type_ = spa::sys::SPA_DATA_MemFd;
                data.flags = spa::sys::SPA_DATA_FLAG_READWRITE;
                data.fd = fd;
                data.mapoffset = 0;
                data.maxsize = len as u32;
                data.data = shm.ptr.cast();
                // SAFETY: PipeWire gives each data its chunk.
                if let Some(chunk) = unsafe { data.chunk.as_mut() } {
                    chunk.offset = 0;
                    chunk.stride = stride as i32;
                    chunk.size = 0;
                    chunk.flags = 0;
                }
                Memory::Shm(shm)
            }
        };
        self.memory.insert(buffer as usize, memory);
    }

    /// `buffer` is gone (`remove_buffer`): its memory goes.
    pub fn remove(&mut self, buffer: *mut pipewire::sys::pw_buffer) {
        self.memory.remove(&(buffer as usize));
    }
}

/// The modifiers a negotiated format offers, if it has any: whether it's
/// fixed (a value) or left to pick from (a choice, even of one), and which.
pub fn format_modifiers(format: &spa::pod::Pod) -> Option<(bool, Vec<u64>)> {
    use spa::pod::{deserialize::PodDeserializer, ChoiceValue, Value};
    use spa::utils::ChoiceEnum;
    let (_, value) = PodDeserializer::deserialize_any_from(format.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    let property = object
        .properties
        .iter()
        .find(|p| p.key == spa::sys::SPA_FORMAT_VIDEO_modifier)?;
    match &property.value {
        Value::Long(modifier) => Some((true, vec![*modifier as u64])),
        Value::Choice(ChoiceValue::Long(choice)) => match &choice.1 {
            ChoiceEnum::None(modifier) => Some((true, vec![*modifier as u64])),
            // A choice is left for Pila to fix even with one value in it
            // (`DONT_FIXATE`): the stream's format isn't set until then.
            ChoiceEnum::Enum {
                default,
                alternatives,
            } => {
                let mut all: Vec<u64> = Vec::new();
                for modifier in std::iter::once(*default).chain(alternatives.iter().copied()) {
                    if !all.contains(&(modifier as u64)) {
                        all.push(modifier as u64);
                    }
                }
                Some((false, all))
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Shm;

    /// The app sharing gets the memfd: shrinking it under Pila, still
    /// writing frames into its mapping, would end Pila with SIGBUS.
    #[test]
    fn a_shared_memory_buffer_cant_be_shrunk_or_grown() {
        let (shm, raw) = Shm::new(4096).expect("a memfd");
        let fd = raw as libc::c_int;
        // SAFETY: plain syscalls on a fd `shm` keeps open.
        let (shrunk, grown) = unsafe { (libc::ftruncate(fd, 0), libc::ftruncate(fd, 8192)) };
        assert_eq!(shrunk, -1, "shrunk");
        assert_eq!(grown, -1, "grown");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
        drop(shm);
    }
}
