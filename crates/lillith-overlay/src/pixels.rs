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

/// Alpha at or above which a pixel is kept by the X11 *bounding* shape when
/// no compositing manager blends per-pixel alpha (hard-edged fallback).
pub const SHAPE_ALPHA: u8 = 128;

/// Writes premultiplied RGBA `src` into `dst` as fully opaque `0xFFRRGGBB`
/// words with the colour un-premultiplied, for a window that is clipped by
/// an X Shape bounding mask instead of being alpha-blended (no compositor,
/// or no 32-bit visual). Pixels below `threshold` are outside the shape and
/// written as `0`. Without un-premultiplying, the anti-aliased edge would
/// show as a dark fringe.
pub fn rgba_to_opaque_u32(src: &[u8], dst: &mut [u32], threshold: u8) {
    for (s, d) in src.chunks_exact(4).zip(dst.iter_mut()) {
        let a = s[3];
        *d = if a < threshold.max(1) {
            0
        } else {
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            u32::from_be_bytes([0xFF, un(s[0]), un(s[1]), un(s[2])])
        };
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

    #[test]
    fn opaque_fallback_unpremultiplies_and_drops_faint_pixels() {
        // Half-alpha premultiplied (100, 50, 0) is straight (200, 100, 0).
        let src = [100u8, 50, 0, 128, 9, 9, 9, 20, 0, 0, 0, 0];
        let mut w = [1u32; 3];
        rgba_to_opaque_u32(&src, &mut w, SHAPE_ALPHA);
        assert_eq!(w[0] >> 24, 0xFF);
        assert!(((w[0] >> 16) & 0xFF).abs_diff(199) <= 1);
        assert_eq!(w[1], 0, "below threshold");
        assert_eq!(w[2], 0, "fully transparent");
    }
}
