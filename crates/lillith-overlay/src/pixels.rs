//! Canvas → window buffer conversion.
//!
//! `tiny-skia` pixmaps are premultiplied RGBA8. Both `wl_shm` `Argb8888`
//! and an ARGB X11 visual (softbuffer) want premultiplied `0xAARRGGBB`
//! words, i.e. little-endian bytes `B, G, R, A`.

/// One premultiplied RGBA pixel → `0xAARRGGBB`, scaled by `opacity` (0..=1).
#[inline]
pub fn argb(rgba: [u8; 4], opacity: u8) -> u32 {
    let [r, g, b, a] = rgba;
    let (r, g, b, a) = if opacity == 255 {
        (r, g, b, a)
    } else {
        let m = |c: u8| ((c as u16 * opacity as u16 + 127) / 255) as u8;
        (m(r), m(g), m(b), m(a))
    };
    u32::from_be_bytes([a, r, g, b])
}

/// Opacity as a byte from a 0..=1 float.
pub fn opacity_byte(o: f32) -> u8 {
    (o.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Writes premultiplied RGBA `src` into `dst` as `Argb8888` little-endian bytes
/// (`wl_shm`). `dst.len()` must be `src.len()`.
pub fn rgba_to_argb8888_le(src: &[u8], dst: &mut [u8], opacity: u8) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let v = argb([s[0], s[1], s[2], s[3]], opacity);
        d.copy_from_slice(&v.to_le_bytes());
    }
}

/// Writes premultiplied RGBA `src` into `dst` as `0xAARRGGBB` words (softbuffer).
pub fn rgba_to_argb_u32(src: &[u8], dst: &mut [u32], opacity: u8) {
    for (s, d) in src.chunks_exact(4).zip(dst.iter_mut()) {
        *d = argb([s[0], s[1], s[2], s[3]], opacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_order_and_opacity() {
        let src = [10u8, 20, 30, 255, 0, 0, 0, 0];
        let mut le = [0u8; 8];
        rgba_to_argb8888_le(&src, &mut le, 255);
        assert_eq!(le, [30, 20, 10, 255, 0, 0, 0, 0]);
        let mut w = [0u32; 2];
        rgba_to_argb_u32(&src, &mut w, 255);
        assert_eq!(w, [0xFF0A141E, 0]);
        // Half opacity keeps the pixel premultiplied.
        rgba_to_argb_u32(&src, &mut w, 128);
        assert_eq!(w[0] >> 24, 128);
        assert_eq!((w[0] >> 16) & 0xFF, 5);
        assert_eq!(opacity_byte(2.0), 255);
        assert_eq!(opacity_byte(0.0), 0);
    }
}
