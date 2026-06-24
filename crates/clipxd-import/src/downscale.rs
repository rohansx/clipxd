//! RGBA frame → veyo `Cell` grid.
//!
//! veyo-core consumes pre-downscaled 8×8 luminance [`Cell`](veyo_core::Cell)s, not pixels
//! (privacy + cost: pixels never enter the codec). The reference downscaler lives in
//! `veyo-capture`, which pulls a platform capture stack (`xcap`) we don't want here — so
//! this is an independent, functionally-equivalent reimplementation (both Apache-2.0).

use veyo_core::{Cell, CELL_LEN, CELL_SIDE};

/// Box-average an RGBA frame into a `cols × rows` grid of 8×8 luma cells (Rec.601 luma).
pub fn rgba_to_cells(rgba: &[u8], w: u32, h: u32, cols: u8, rows: u8) -> Vec<Cell> {
    let cols = cols.max(1) as usize;
    let rows = rows.max(1) as usize;
    let w = w as usize;
    let h = h as usize;
    let cw = (w / cols).max(1);
    let ch = (h / rows).max(1);

    let mut out = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            out.push(region_to_cell(rgba, w, col * cw, row * ch, cw, ch));
        }
    }
    out
}

fn region_to_cell(rgba: &[u8], stride: usize, x0: usize, y0: usize, rw: usize, rh: usize) -> Cell {
    let mut cell = [0u8; CELL_LEN];
    let bw = (rw / CELL_SIDE).max(1);
    let bh = (rh / CELL_SIDE).max(1);

    for cy in 0..CELL_SIDE {
        for cx in 0..CELL_SIDE {
            let px0 = x0 + cx * bw;
            let py0 = y0 + cy * bh;
            let mut sum: u32 = 0;
            let mut count: u32 = 0;
            for dy in 0..bh {
                for dx in 0..bw {
                    let pi = ((py0 + dy) * stride + (px0 + dx)) * 4;
                    if pi + 3 < rgba.len() {
                        let r = rgba[pi] as u32;
                        let g = rgba[pi + 1] as u32;
                        let b = rgba[pi + 2] as u32;
                        sum += (299 * r + 587 * g + 114 * b) / 1000;
                        count += 1;
                    }
                }
            }
            cell[cy * CELL_SIDE + cx] = sum.checked_div(count).unwrap_or(0) as u8;
        }
    }
    cell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_frames_map_to_uniform_cells() {
        let white = vec![255u8; 64 * 64 * 4];
        let cells = rgba_to_cells(&white, 64, 64, 1, 1);
        assert_eq!(cells.len(), 1);
        assert!(cells[0].iter().all(|&v| v == 255));

        let black = vec![0u8; 64 * 64 * 4];
        let cells = rgba_to_cells(&black, 64, 64, 8, 8);
        assert_eq!(cells.len(), 64);
        assert!(cells.iter().all(|c| c.iter().all(|&v| v == 0)));
    }

    #[test]
    fn grid_yields_cols_times_rows_cells() {
        let mid = vec![128u8; 1280 * 720 * 4];
        assert_eq!(rgba_to_cells(&mid, 1280, 720, 8, 8).len(), 64);
    }
}
