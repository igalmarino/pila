//! Which monitor is on a connector, from its EDID: make, model and serial
//! for clients (`wl_output`, wlr-output-management) and for `[[output]]`
//! sections that name a monitor rather than a connector.
//!
//! Only the 128-byte base block is read: it has all of this, so pila needs no
//! EDID library (`smithay-drm-extras`' binds an old libdisplay-info). The make
//! stays the three-letter PNP code (`LEN`): the full name would take a table.
//! Serials identify a person's hardware: they aren't logged.

/// The EDID's base block.
const BLOCK: usize = 128;
const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
/// Where its four 18-byte descriptors start.
const DESCRIPTORS: usize = 54;
const DESCRIPTOR: usize = 18;
/// Display descriptor tags.
const TAG_SERIAL: u8 = 0xff;
const TAG_TEXT: u8 = 0xfe;
const TAG_NAME: u8 = 0xfc;

/// A monitor, as its EDID (or a test's output spec) describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
}

impl Monitor {
    /// Whether an `[[output]]` section's name is this monitor: its make and
    /// model. Any monitor of that model answers to it.
    pub fn is_model(&self, name: &str) -> bool {
        name == format!("{} {}", self.make, self.model)
    }

    /// Whether a section's name is this very monitor: make, model and serial.
    pub fn is_exactly(&self, name: &str) -> bool {
        self.serial
            .as_ref()
            .is_some_and(|serial| name == format!("{} {} {serial}", self.make, self.model))
    }
}

/// The text of a display descriptor: up to a newline, printable ASCII only.
fn descriptor_text(descriptor: &[u8]) -> Option<String> {
    let text: String = descriptor[5..]
        .iter()
        .take_while(|byte| **byte != b'\n')
        .filter(|byte| byte.is_ascii_graphic() || **byte == b' ')
        .map(|byte| *byte as char)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The monitor in an EDID; `None` if it isn't a valid one.
pub fn parse(edid: &[u8]) -> Option<Monitor> {
    let block = edid.get(..BLOCK)?;
    let checksum = block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    if block[..HEADER.len()] != HEADER || checksum != 0 {
        return None;
    }

    // Three letters in five bits each, A being 1.
    let code = u16::from_be_bytes([block[8], block[9]]);
    let make: String = [10, 5, 0]
        .iter()
        .map(|shift| ((code >> shift) & 0x1f) as u8)
        .filter(|letter| (1..=26).contains(letter))
        .map(|letter| (b'A' + letter - 1) as char)
        .collect();
    if make.len() != 3 {
        return None;
    }

    let (mut name, mut text, mut serial) = (None, None, None);
    for descriptor in block[DESCRIPTORS..DESCRIPTORS + 4 * DESCRIPTOR].chunks(DESCRIPTOR) {
        // Timings start with a non-zero pixel clock.
        if descriptor[..3] != [0, 0, 0] {
            continue;
        }
        match descriptor[3] {
            TAG_NAME => name = descriptor_text(descriptor),
            TAG_SERIAL => serial = descriptor_text(descriptor),
            // Laptop panels often have no name, and their part number here;
            // when there are two texts the first is the panel's maker.
            TAG_TEXT => text = descriptor_text(descriptor).or(text),
            _ => {}
        }
    }
    let product = u16::from_le_bytes([block[10], block[11]]);
    let model = name.or(text).unwrap_or_else(|| format!("0x{product:04X}"));
    let number = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
    let serial = serial.or_else(|| (number != 0).then(|| number.to_string()));
    Some(Monitor {
        make,
        model,
        serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An EDID from make LEN, product 0x63A1, with these display descriptors.
    fn edid(number: u32, descriptors: &[(u8, &str)]) -> Vec<u8> {
        let mut edid = vec![0; BLOCK];
        edid[..8].copy_from_slice(&HEADER);
        // L, E, N.
        edid[8..10].copy_from_slice(&((12u16 << 10) | (5 << 5) | 14).to_be_bytes());
        edid[10..12].copy_from_slice(&0x63a1u16.to_le_bytes());
        edid[12..16].copy_from_slice(&number.to_le_bytes());
        // The first descriptor is a timing: a non-zero pixel clock.
        edid[DESCRIPTORS] = 1;
        for (i, (tag, text)) in descriptors.iter().enumerate() {
            let at = DESCRIPTORS + (i + 1) * DESCRIPTOR;
            edid[at + 3] = *tag;
            edid[at + 5..at + DESCRIPTOR].fill(b' ');
            edid[at + 5..at + 5 + text.len()].copy_from_slice(text.as_bytes());
            if text.len() < 13 {
                edid[at + 5 + text.len()] = b'\n';
            }
        }
        edid[BLOCK - 1] = 0u8.wrapping_sub(edid.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
        edid
    }

    #[test]
    fn a_monitor_gives_its_make_name_and_serial() {
        let monitor = parse(&edid(7, &[(TAG_SERIAL, "ABC123"), (TAG_NAME, "P27h-30")])).unwrap();
        assert_eq!(
            monitor,
            Monitor {
                make: "LEN".into(),
                model: "P27h-30".into(),
                serial: Some("ABC123".into()),
            }
        );
    }

    #[test]
    fn a_laptop_panel_without_a_name_gives_its_part_number() {
        let monitor = parse(&edid(7, &[(TAG_TEXT, "X140NVFC R0")])).unwrap();
        assert_eq!(monitor.model, "X140NVFC R0");
        // No serial text: the number stands in.
        assert_eq!(monitor.serial.as_deref(), Some("7"));
    }

    #[test]
    fn of_two_texts_the_second_is_the_part_number() {
        let monitor = parse(&edid(0, &[(TAG_TEXT, "AUO"), (TAG_TEXT, "B140HAN")])).unwrap();
        assert_eq!(monitor.model, "B140HAN");
    }

    #[test]
    fn a_name_wins_over_a_text() {
        let monitor = parse(&edid(0, &[(TAG_TEXT, "text"), (TAG_NAME, "name")])).unwrap();
        assert_eq!(monitor.model, "name");
    }

    #[test]
    fn a_text_filling_its_descriptor_has_no_newline() {
        let monitor = parse(&edid(0, &[(TAG_NAME, "THIRTEEN CHAR")])).unwrap();
        assert_eq!(monitor.model, "THIRTEEN CHAR");
    }

    #[test]
    fn without_a_name_the_product_code_is_the_model() {
        let monitor = parse(&edid(0, &[])).unwrap();
        assert_eq!(monitor.model, "0x63A1");
        assert_eq!(monitor.serial, None);
    }

    #[test]
    fn whats_not_an_edid_gives_no_monitor() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0; BLOCK]), None);
        let good = edid(0, &[(TAG_NAME, "name")]);
        assert_eq!(parse(&good[..BLOCK - 1]), None);
        let mut bad_checksum = good.clone();
        bad_checksum[20] ^= 1;
        assert_eq!(parse(&bad_checksum), None);
        let mut bad_header = good;
        bad_header[0] = 1;
        bad_header[BLOCK - 1] = bad_header[BLOCK - 1].wrapping_sub(1);
        assert_eq!(parse(&bad_header), None);
    }

    #[test]
    fn a_make_code_out_of_range_gives_no_monitor() {
        let mut edid = edid(0, &[]);
        edid[8] = 0;
        edid[9] = 0;
        edid[BLOCK - 1] = 0;
        edid[BLOCK - 1] = 0u8.wrapping_sub(edid.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
        assert_eq!(parse(&edid), None);
    }

    #[test]
    fn extension_blocks_dont_matter() {
        let mut long = edid(0, &[(TAG_NAME, "name")]);
        long.extend_from_slice(&[0xab; BLOCK]);
        assert_eq!(parse(&long).unwrap().model, "name");
    }

    #[test]
    fn a_monitor_answers_to_its_model_and_to_itself() {
        let monitor = Monitor {
            make: "LEN".into(),
            model: "P27h-30".into(),
            serial: Some("ABC".into()),
        };
        assert!(monitor.is_model("LEN P27h-30"));
        assert!(!monitor.is_model("LEN P27h-31"));
        assert!(monitor.is_exactly("LEN P27h-30 ABC"));
        assert!(!monitor.is_exactly("LEN P27h-30"));
    }
}
