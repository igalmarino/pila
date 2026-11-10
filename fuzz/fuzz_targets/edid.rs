//! A monitor's EDID blob (`edid::parse`), as a connector's property gives it.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|edid: &[u8]| {
    let _ = pila::edid::parse(edid);
});
