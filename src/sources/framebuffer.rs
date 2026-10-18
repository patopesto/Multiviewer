/// A software framebuffer that protocol producers blit incremental updates
/// into. Protocol-agnostic: VNC feeds RFB rectangles, RDP desktop updates.
pub struct Framebuffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32, bytes_per_pixel: usize) -> Self {
        return Self {
            data: vec![0; buffer_size(width, height, bytes_per_pixel)],
            width,
            height,
            bytes_per_pixel,
        };
    }

    pub fn width(&self) -> u32 {
        return self.width;
    }

    pub fn height(&self) -> u32 {
        return self.height;
    }

    /// Reallocate for a new resolution, keeping the pixels that still fit. A size
    /// change must not wipe a desktop already painted: the server has to repaint
    /// the whole screen after a real resize, so the stale overlap is overwritten
    /// anyway, while a spurious size announcement would otherwise blank the tile.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        let bpp = self.bytes_per_pixel;
        let mut data = vec![0; buffer_size(width, height, bpp)];
        let copy_bytes = self.width.min(width) as usize * bpp;
        let src_stride = self.width as usize * bpp;
        let dst_stride = width as usize * bpp;
        for row in 0..self.height.min(height) as usize {
            let src = row * src_stride;
            let dst = row * dst_stride;
            data[dst..dst + copy_bytes].copy_from_slice(&self.data[src..src + copy_bytes]);
        }
        self.data = data;
        self.width = width;
        self.height = height;
    }

    /// Number of bytes a snapshot of this framebuffer occupies.
    pub fn byte_len(&self) -> usize {
        return self.data.len();
    }

    /// Copy a tightly packed `width`x`height` pixel rectangle to (`x`, `y`),
    /// clipped to the framebuffer. `pixels` must hold `width * height * bpp`.
    pub fn blit(&mut self, x: u32, y: u32, width: u32, height: u32, pixels: &[u8]) {
        let bpp = self.bytes_per_pixel;
        let src_stride = width as usize * bpp;
        if src_stride * height as usize > pixels.len() {
            return;
        }
        let x_end = x.saturating_add(width).min(self.width);
        let y_end = y.saturating_add(height).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let row_bytes = (x_end - x) as usize * bpp;
        let dst_stride = self.width as usize * bpp;
        for row in y..y_end {
            let src = (row - y) as usize * src_stride;
            let dst = row as usize * dst_stride + x as usize * bpp;
            self.data[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
        }
    }

    /// Copy the `width`x`height` region at (`src_x`, `src_y`) to
    /// (`dst_x`, `dst_y`), the RFB CopyRect operation. Overlapping regions are
    /// handled: rows are copied in the direction that avoids clobbering a row
    /// before it is read.
    pub fn copy_rect(
        &mut self,
        dst_x: u32,
        dst_y: u32,
        src_x: u32,
        src_y: u32,
        width: u32,
        height: u32,
    ) {
        let bpp = self.bytes_per_pixel;
        let width = width
            .min(self.width.saturating_sub(src_x))
            .min(self.width.saturating_sub(dst_x));
        let height = height
            .min(self.height.saturating_sub(src_y))
            .min(self.height.saturating_sub(dst_y));
        if width == 0 || height == 0 {
            return;
        }
        let stride = self.width as usize * bpp;
        let row_bytes = width as usize * bpp;
        for i in 0..height {
            let row = if dst_y <= src_y { i } else { height - 1 - i };
            let src = (src_y + row) as usize * stride + src_x as usize * bpp;
            let dst = (dst_y + row) as usize * stride + dst_x as usize * bpp;
            self.data.copy_within(src..src + row_bytes, dst);
        }
    }

    /// Copy the whole framebuffer into `dst`, reusing its allocation.
    pub fn snapshot_into(&self, dst: &mut Vec<u8>) {
        dst.clear();
        dst.extend_from_slice(&self.data);
    }
}

fn buffer_size(width: u32, height: u32, bytes_per_pixel: usize) -> usize {
    return width as usize * height as usize * bytes_per_pixel;
}

#[cfg(test)]
mod tests {
    use super::*;

    const BPP: usize = 4;

    /// A 4x4 framebuffer whose pixel value encodes its position.
    fn grid() -> Framebuffer {
        let mut fb = Framebuffer::new(4, 4, BPP);
        for y in 0..4u32 {
            for x in 0..4u32 {
                fb.blit(x, y, 1, 1, &[x as u8, y as u8, 0, 0]);
            }
        }
        return fb;
    }

    fn pixel(fb: &Framebuffer, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * fb.width as usize + x as usize) * BPP;
        return fb.data[offset..offset + BPP].try_into().unwrap();
    }

    #[test]
    fn blit_writes_at_the_requested_offset() {
        let mut fb = Framebuffer::new(4, 4, BPP);
        fb.blit(1, 2, 2, 1, &[9, 9, 9, 9, 8, 8, 8, 8]);
        assert_eq!(pixel(&fb, 1, 2), [9, 9, 9, 9]);
        assert_eq!(pixel(&fb, 2, 2), [8, 8, 8, 8]);
        assert_eq!(pixel(&fb, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn blit_clips_to_the_framebuffer() {
        let mut fb = Framebuffer::new(4, 4, BPP);
        // A 3x3 rect at (3,3) keeps only its top-left pixel.
        fb.blit(3, 3, 3, 3, &[1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3,
                             4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6,
                             7, 7, 7, 7, 8, 8, 8, 8, 9, 9, 9, 9]);
        assert_eq!(pixel(&fb, 3, 3), [1, 1, 1, 1]);
    }

    #[test]
    fn blit_ignores_short_buffers_and_out_of_bounds_rects() {
        let mut fb = Framebuffer::new(4, 4, BPP);
        fb.blit(0, 0, 2, 2, &[1, 1, 1, 1]); // too short
        assert_eq!(pixel(&fb, 0, 0), [0, 0, 0, 0]);
        fb.blit(4, 0, 1, 1, &[1, 1, 1, 1]); // entirely outside
        assert_eq!(pixel(&fb, 3, 0), [0, 0, 0, 0]);
    }

    /// Scrolling down (destination below source) must not clobber source rows
    /// before they are read.
    #[test]
    fn copy_rect_handles_overlap_both_ways() {
        let mut fb = grid();
        fb.copy_rect(0, 1, 0, 0, 4, 3);
        assert_eq!(pixel(&fb, 0, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&fb, 2, 3), [2, 2, 0, 0]);

        let mut fb = grid();
        fb.copy_rect(0, 0, 0, 1, 4, 3);
        assert_eq!(pixel(&fb, 0, 0), [0, 1, 0, 0]);
        assert_eq!(pixel(&fb, 3, 2), [3, 3, 0, 0]);
    }

    #[test]
    fn copy_rect_clips_to_the_framebuffer() {
        let mut fb = grid();
        fb.copy_rect(3, 3, 0, 0, 2, 2);
        assert_eq!(pixel(&fb, 3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn resize_changes_the_buffer_size() {
        let mut fb = grid();
        fb.resize(2, 2);
        assert_eq!(fb.byte_len(), 2 * 2 * BPP);
        assert_eq!(pixel(&fb, 1, 1), [1, 1, 0, 0]);

        fb.resize(8, 4);
        assert_eq!(fb.byte_len(), 8 * 4 * BPP);
        assert_eq!(pixel(&fb, 7, 3), [0, 0, 0, 0]);
    }

    /// A size change keeps the pixels that still fit, so a spurious size
    /// announcement cannot blank a desktop that was already painted.
    #[test]
    fn resize_keeps_the_overlapping_pixels() {
        let mut fb = grid();
        fb.resize(6, 6);
        assert_eq!(pixel(&fb, 3, 3), [3, 3, 0, 0]);
        assert_eq!(pixel(&fb, 5, 5), [0, 0, 0, 0]);

        fb.resize(2, 2);
        assert_eq!(pixel(&fb, 1, 1), [1, 1, 0, 0]);

        fb.resize(2, 2);
        assert_eq!(pixel(&fb, 1, 1), [1, 1, 0, 0]);
    }

    #[test]
    fn snapshot_matches_the_buffer() {
        let fb = grid();
        let mut out = Vec::new();
        fb.snapshot_into(&mut out);
        assert_eq!(out, fb.data);
        assert_eq!(out.len(), fb.byte_len());
    }
}
