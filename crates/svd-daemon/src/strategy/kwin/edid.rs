//! EDID 1.4 binary generator — produces a 128-byte base block for a virtual display.
//!
//! Ported from `src/edid/generator.py` and `src/edid/timing.py` in the Python reference
//! implementation. Only the base block is emitted (no CEA-861 extension), and HDR is
//! intentionally omitted (the Python reference notes HDR causes freezes).

/// Generate a 128-byte EDID 1.4 binary blob for a virtual display.
///
/// The blob is suitable for injection into debugfs so the kernel presents a
/// fake connected display at the requested resolution and refresh rate.
///
/// # Arguments
/// * `width`      - Horizontal active pixels
/// * `height`     - Vertical active pixels
/// * `refresh_hz` - Refresh rate in Hz (e.g. 60)
pub fn generate(width: u32, height: u32, refresh_hz: u32) -> Vec<u8> {
    let mut edid = vec![0u8; 128];

    // ── Header (bytes 0–7) ────────────────────────────────────────────────────
    edid[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);

    // ── Manufacturer ID — "VRT" (Virtual) encoded as 3×5-bit packed big-endian ──
    // Each letter: value = char - 'A' + 1  →  V=22, R=18, T=20
    // Packed: (22 << 10) | (18 << 5) | 20  = 0x5994
    // byte[8] = high byte = 0x59, byte[9] = low byte = 0x94
    let mfr_id = encode_manufacturer_id(b'V', b'R', b'T');
    edid[8] = (mfr_id >> 8) as u8;
    edid[9] = (mfr_id & 0xFF) as u8;

    // Product code (little-endian) — 0x5344 ("SD" for Standard Display)
    let product_code: u16 = 0x5344;
    edid[10] = (product_code & 0xFF) as u8;
    edid[11] = (product_code >> 8) as u8;

    // Serial number (little-endian) — unique per resolution/refresh
    let serial: u32 = (width << 16) | (height << 4) | (refresh_hz & 0x0F);
    edid[12] = (serial & 0xFF) as u8;
    edid[13] = ((serial >> 8) & 0xFF) as u8;
    edid[14] = ((serial >> 16) & 0xFF) as u8;
    edid[15] = ((serial >> 24) & 0xFF) as u8;

    // Week / year of manufacture
    edid[16] = 1; // Week 1
    edid[17] = 33; // 1990 + 33 = 2023

    // ── EDID version 1.4 (bytes 18–19) ───────────────────────────────────────
    edid[18] = 1; // Version 1
    edid[19] = 4; // Revision 4

    // ── Video input definition — digital, 8-bit colour depth, DisplayPort ────
    edid[20] = 0xA5;

    // ── Screen size in cm ─────────────────────────────────────────────────────
    // Approximation from diagonal at 96 dpi; matches Python reference.
    let (h_size_cm, v_size_cm) = screen_size_cm(width, height);
    edid[21] = h_size_cm.min(255) as u8;
    edid[22] = v_size_cm.min(255) as u8;

    // Display gamma (2.2)
    edid[23] = 220;

    // Feature support — RGB 4:4:4, sRGB standard, preferred timing, continuous freq.
    edid[24] = 0x1E;

    // ── Colour characteristics (bytes 25–34) ─────────────────────────────────
    // sRGB primaries; copied verbatim from Python reference.
    edid[25..35].copy_from_slice(&[0xEE, 0x91, 0xA3, 0x54, 0x4C, 0x99, 0x26, 0x0F, 0x50, 0x54]);

    // ── Established timings (bytes 35–37) — none declared ───────────────────
    edid[35] = 0x00;
    edid[36] = 0x00;
    edid[37] = 0x00;

    // ── Standard timings (bytes 38–53) — all unused ──────────────────────────
    for i in 0..8 {
        edid[38 + i * 2] = 0x01;
        edid[38 + i * 2 + 1] = 0x01;
    }

    // ── Detailed Timing Descriptor 1 (bytes 54–71) ───────────────────────────
    let h_active = width;
    let v_active = height;

    let h_blank: u32 = 80.max(width * 8 / 100); // max(80, int(width * 0.08))
    let h_total: u32 = h_active + h_blank;

    // Python: max(23, int(height * 0.025)) — integer arithmetic safe
    let v_blank_estimate: u32 = 23.max(height / 40);

    // Pixel clock over 64 bits to avoid overflow for 4K+ resolutions
    let pixel_clock_hz: u64 =
        h_total as u64 * (v_active as u64 + v_blank_estimate as u64) * refresh_hz as u64;

    let v_blank: u32 = {
        let vb = pixel_clock_hz / (h_total as u64 * refresh_hz as u64);
        let vb = vb.saturating_sub(v_active as u64) as u32;
        23.max(vb)
    };

    let pixel_clock_hz: u64 =
        h_total as u64 * (v_active as u64 + v_blank as u64) * refresh_hz as u64;
    let pixel_clock: u16 = (pixel_clock_hz / 10_000).min(65535) as u16;

    // Byte 54–55: pixel clock in units of 10 kHz, little-endian
    edid[54] = (pixel_clock & 0xFF) as u8;
    edid[55] = (pixel_clock >> 8) as u8;

    // H active / H blank
    edid[56] = (h_active & 0xFF) as u8;
    edid[57] = (h_blank & 0xFF) as u8;
    edid[58] = (((h_active >> 8) << 4) | (h_blank >> 8)) as u8;

    // V active / V blank
    edid[59] = (v_active & 0xFF) as u8;
    edid[60] = (v_blank & 0xFF) as u8;
    edid[61] = (((v_active >> 8) << 4) | (v_blank >> 8)) as u8;

    // Sync offsets and widths
    let h_sync_offset: u32 = (h_blank * 2) / 10; // int(h_blank * 0.2)
    let h_sync_width: u32 = (h_blank * 4) / 10; // int(h_blank * 0.4)
    let v_sync_offset: u32 = 2;
    let v_sync_width: u32 = 6;

    edid[62] = (h_sync_offset & 0xFF) as u8;
    edid[63] = (h_sync_width & 0xFF) as u8;
    edid[64] = (((v_sync_offset & 0x0F) << 4) | (v_sync_width & 0x0F)) as u8;
    edid[65] = ((((h_sync_offset >> 8) & 0x03) << 6)
        | (((h_sync_width >> 8) & 0x03) << 4)
        | (((v_sync_offset >> 4) & 0x03) << 2)
        | ((v_sync_width >> 4) & 0x03)) as u8;

    // Image size in mm (derived from screen size in cm × 10)
    let h_size_mm: u32 = h_size_cm * 10;
    let v_size_mm: u32 = v_size_cm * 10;
    edid[66] = (h_size_mm & 0xFF) as u8;
    edid[67] = (v_size_mm & 0xFF) as u8;
    edid[68] = (((h_size_mm >> 8) << 4) | (v_size_mm >> 8)) as u8;

    edid[69] = 0; // H border
    edid[70] = 0; // V border
    edid[71] = 0x18; // Non-interlaced, digital separate sync

    // ── Display product name descriptor (bytes 72–89) ────────────────────────
    // Tag 0xFC = monitor name. Per EDID 1.4 spec: name string + 0x0A (LF) terminator
    // + space padding to fill 13 bytes total.
    // "Display" (7) + LF (1) + 5 spaces (5) = 13 bytes.
    let name = b"Display\n     "; // 13 bytes: name + 0x0A + padding
    edid[72] = 0x00;
    edid[73] = 0x00;
    edid[74] = 0x00;
    edid[75] = 0xFC;
    edid[76] = 0x00;
    edid[77..90].copy_from_slice(name);

    // ── Display range limits descriptor (bytes 90–107) ───────────────────────
    let min_v_rate: u32 = 24.max(refresh_hz.saturating_sub(20));
    let max_v_rate: u32 = refresh_hz + 20;
    edid[90] = 0x00;
    edid[91] = 0x00;
    edid[92] = 0x00;
    edid[93] = 0xFD; // Tag: range limits
    edid[94] = 0x00;
    edid[95] = min_v_rate.min(255) as u8;
    edid[96] = max_v_rate.min(255) as u8;
    edid[97] = 30; // Min H rate (kHz)
    edid[98] = 160; // Max H rate (kHz)
    edid[99] = 220; // Max pixel clock / 10 MHz → 2200 MHz
    edid[100] = 0x00;
    edid[101] = 0x0A;
    edid[102] = 0x20;
    edid[103] = 0x20;
    edid[104] = 0x20;
    edid[105] = 0x20;
    edid[106] = 0x20;
    edid[107] = 0x20;

    // ── Dummy descriptor (bytes 108–125) ─────────────────────────────────────
    edid[108] = 0x00;
    edid[109] = 0x00;
    edid[110] = 0x00;
    edid[111] = 0x10; // Tag: dummy
    edid[112] = 0x00;
    // bytes 113–125 remain 0x00

    // ── Extension count (byte 126) — zero: base block only ──────────────────
    edid[126] = 0x00;

    // ── Checksum (byte 127) ──────────────────────────────────────────────────
    // The sum of all 128 bytes must equal 0 mod 256.
    edid[127] = checksum(&edid[0..127]);

    edid
}

/// Encode a 3-letter manufacturer ID into a 16-bit big-endian value.
/// Each letter is encoded as `c - b'A' + 1` and packed into 5 bits.
/// Layout: bits [14:10] = c1, [9:5] = c2, [4:0] = c3; bit 15 is always 0.
fn encode_manufacturer_id(c1: u8, c2: u8, c3: u8) -> u16 {
    let v1 = (c1 - b'A' + 1) as u16;
    let v2 = (c2 - b'A' + 1) as u16;
    let v3 = (c3 - b'A' + 1) as u16;
    (v1 << 10) | (v2 << 5) | v3
}

/// Compute the EDID checksum byte: `(256 - (sum(data) % 256)) % 256`.
fn checksum(data: &[u8]) -> u8 {
    let sum: u32 = data.iter().map(|&b| b as u32).sum();
    ((256 - (sum % 256)) % 256) as u8
}

/// Approximate the screen size in cm from resolution at 96 dpi.
/// Returns `(h_size_cm, v_size_cm)`.
fn screen_size_cm(width: u32, height: u32) -> (u32, u32) {
    // diagonal_inches = sqrt(width² + height²) / 96
    // aspect_ratio    = width / height
    // h_size_cm = diagonal_inches * 2.54 / sqrt(1 + (1/aspect_ratio)²)
    //           = diagonal_inches * 2.54 * sin(atan(aspect_ratio))   (equivalent)
    // Computed in floating point for accuracy; only used for display metadata.
    let w = width as f64;
    let h = height as f64;
    let diagonal_inches = (w * w + h * h).sqrt() / 96.0;
    let aspect = w / h;
    let h_size_cm = (diagonal_inches * 2.54) / (1.0 + (1.0 / aspect).powi(2)).sqrt();
    let v_size_cm = h_size_cm / aspect;
    (h_size_cm as u32, v_size_cm as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_length_is_128() {
        let edid = generate(1920, 1080, 60);
        assert_eq!(edid.len(), 128);
    }

    #[test]
    fn edid_header_is_valid() {
        let edid = generate(1920, 1080, 60);
        assert_eq!(
            &edid[0..8],
            &[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]
        );
    }

    #[test]
    fn edid_checksum_is_valid() {
        let edid = generate(1920, 1080, 60);
        let sum: u32 = edid.iter().map(|b| *b as u32).sum();
        assert_eq!(
            sum % 256,
            0,
            "EDID checksum should make the whole block sum to 0 mod 256"
        );
    }

    #[test]
    fn edid_version_is_1_4() {
        let edid = generate(1920, 1080, 60);
        assert_eq!(edid[18], 1, "EDID version should be 1");
        assert_eq!(edid[19], 4, "EDID revision should be 4");
    }
}
