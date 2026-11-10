//! A gamma ramp as a client sends it (`gamma_control::read_ramp`): any bytes,
//! for a ramp of any size, through a file or a pipe its client may keep open
//! (it must never block Pila).
#![no_main]

use std::{fs::File, io::Write, os::fd::FromRawFd};

use libfuzzer_sys::fuzz_target;

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    /// Entries per channel, as Pila tells the client (bounded like a real
    /// CRTC's).
    size: u16,
    /// A pipe (else a file), and whether its writer stays open.
    pipe: bool,
    keep_open: bool,
    data: Vec<u8>,
}

fuzz_target!(|input: Input| {
    let bytes = usize::from(input.size % 4097) * 3 * 2;
    let mut ramp = vec![0u8; bytes];
    let (file, _writer) = source(&input);
    let _ = pila::gamma_control::read_ramp(file, &mut ramp);
});

/// What Pila reads from: a pipe with the data in it (as much as fits), and
/// its writer if it stays open while Pila reads; or a memory file.
fn source(input: &Input) -> (File, Option<File>) {
    if input.pipe {
        let mut fds = [0; 2];
        // SAFETY: `fds` is two ints, as pipe2 wants.
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }, 0);
        // SAFETY: both fds were just made and are owned here.
        let (read, mut write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        let _ = write.write(&input.data);
        (read, input.keep_open.then_some(write))
    } else {
        // SAFETY: a name and flags, as memfd_create wants.
        let fd = unsafe { libc::memfd_create(c"ramp".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        // SAFETY: just made and owned here.
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(&input.data).unwrap();
        // Read from the start, as a client hands it over.
        // SAFETY: a valid fd.
        unsafe { libc::lseek(fd, 0, libc::SEEK_SET) };
        (file, None)
    }
}
