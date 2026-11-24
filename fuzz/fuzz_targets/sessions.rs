//! A `sessions.json` as Pila reads it back (`session_management::
//! parse_sessions`): any file may be there, and a saved size or place is
//! restored only if a window could have had it (`Saved::rect`), then fitted
//! to a monitor (`fit_in_area`) without overflowing.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pila::session_management::{fit_in_area, parse_sessions};

fuzz_target!(|text: &str| {
    let Ok(sessions) = parse_sessions(text) else {
        return;
    };
    for saved in sessions.values().flat_map(|session| session.saved()) {
        let Some(rect) = saved.rect() else {
            continue;
        };
        // A monitor, and one far up and left of the first.
        let areas = [((0, 0), (1920, 1080)), ((-3840, -2160), (3840, 2160))].map(|(loc, size)| {
            let mut area = rect;
            area.loc = loc.into();
            area.size = size.into();
            area
        });
        for area in areas {
            for bar in [0, 30] {
                let _ = fit_in_area(rect.loc, rect.size, bar, area);
            }
        }
    }
});
