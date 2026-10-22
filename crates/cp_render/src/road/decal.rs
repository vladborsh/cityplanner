//! Lane words (T-302d): "BUS" on bus lanes and "TROLLEY" on trolley lanes, every
//! [`SPACING`] metres, as a 5x7 raster of vertex-coloured tiles baked into the lane surface
//! (the lane's own strip is cut out under the word, nothing overlaps). Letters read "up":
//! the first one is nearest to the driver (MUTCD 3B.20). Near mesh only.

use cp_content::{LaneDir, LaneKind};

/// Distance between two words on a lane, metres.
pub const SPACING: f32 = 40.0;
/// Clear lane kept before a junction and after it, metres.
const END_GAP: f32 = 6.0;
/// Rows of a letter, and columns.
const ROWS: usize = 7;
const COLS: usize = 5;
/// Widest pixel across the lane, metres.
const PIXEL_MAX: f32 = 0.5;

/// One word on one lane.
#[derive(Clone, Debug, PartialEq)]
pub struct Decal {
    /// Arc length of the first row boundary (the lowest `s` of the word).
    pub s0: f32,
    /// Length of one pixel row along the road, metres.
    pub pitch: f32,
    pub text: &'static str,
    /// Lateral centre and width of the lane.
    pub offset: f32,
    pub width: f32,
    pub forward: bool,
    pub kind: LaneKind,
}

impl Decal {
    /// Pixel rows along the road (letters and one blank row between two of them).
    pub fn rows(&self) -> usize {
        self.text.len() * (ROWS + 1) - 1
    }

    /// The row boundaries `s0 + j * pitch`, `j` in `0..=rows`.
    pub fn boundaries(&self) -> impl Iterator<Item = f32> + '_ {
        (0..=self.rows()).map(|j| self.s0 + j as f32 * self.pitch)
    }

    /// Row `j` (counted along increasing `s`) if the ring pair starts at `s`.
    pub fn row_at(&self, s: f32) -> Option<usize> {
        let j = ((s - self.s0) / self.pitch).round();
        (j >= 0.0 && (j as usize) < self.rows() && (self.s0 + j * self.pitch - s).abs() < 1e-3)
            .then_some(j as usize)
    }

    /// The pixel width across the lane.
    pub fn pixel(&self) -> f32 {
        (self.width * 0.8 / COLS as f32).min(PIXEL_MAX)
    }

    /// The five columns of row `j` as `(x0, x1, lit)`, in increasing lateral `x`.
    pub fn columns(&self, j: usize) -> [(f32, f32, bool); COLS] {
        // Index along the driving direction: the first letter is nearest to the driver.
        let u = if self.forward { j } else { self.rows() - 1 - j };
        let (letter, within) = (u / (ROWS + 1), u % (ROWS + 1));
        let rows = (within < ROWS)
            .then(|| glyph(self.text.as_bytes()[letter]))
            .flatten();
        let pw = self.pixel();
        std::array::from_fn(|i| {
            // Column `c` counts from the driver's left; the driver's left is +x going forward.
            let c = if self.forward { COLS - 1 - i } else { i };
            let lit = rows.is_some_and(|g| g[ROWS - 1 - within].as_bytes()[c] == b'#');
            let x = self.offset + (i as f32 - (COLS as f32 - 1.0) * 0.5) * pw;
            (x - pw * 0.5, x + pw * 0.5, lit)
        })
    }
}

/// The words of a segment of `length` metres: one centred in each [`SPACING`] cell, none
/// near an end or a crossing (`keep_clear`: arc-length intervals).
pub fn plan(
    lanes: &[(LaneKind, LaneDir, f32, f32)],
    length: f32,
    keep_clear: &[(f32, f32)],
) -> Vec<Decal> {
    let mut out = Vec::new();
    for &(kind, dir, offset, width) in lanes {
        let (text, pitch) = match kind {
            LaneKind::Bus => ("BUS", 0.6),
            LaneKind::Trolley => ("TROLLEY", 0.45),
            _ => continue,
        };
        let rows = text.len() * (ROWS + 1) - 1;
        let long = rows as f32 * pitch;
        for i in 0..(length / SPACING).floor() as usize {
            let c = (i as f32 + 0.5) * SPACING;
            let s0 = c - long * 0.5;
            let s1 = s0 + long;
            if s0 < END_GAP
                || s1 > length - END_GAP
                || keep_clear.iter().any(|z| s1 > z.0 - 1.0 && s0 < z.1 + 1.0)
            {
                continue;
            }
            out.push(Decal {
                s0,
                pitch,
                text,
                offset,
                width,
                forward: dir == LaneDir::Forward,
                kind,
            });
        }
    }
    out
}

/// The 5x7 raster of a letter, top row first.
fn glyph(c: u8) -> Option<&'static [&'static str; ROWS]> {
    Some(match c {
        b'B' => &[
            "####.", "#...#", "#...#", "####.", "#...#", "#...#", "####.",
        ],
        b'U' => &[
            "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        b'S' => &[
            ".####", "#....", "#....", ".###.", "....#", "....#", "####.",
        ],
        b'T' => &[
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
        ],
        b'R' => &[
            "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
        ],
        b'O' => &[
            ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        b'L' => &[
            "#....", "#....", "#....", "#....", "#....", "#....", "#####",
        ],
        b'E' => &[
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ],
        b'Y' => &[
            "#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#..",
        ],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(kind: LaneKind, dir: LaneDir) -> (LaneKind, LaneDir, f32, f32) {
        (kind, dir, 1.75, 3.5)
    }

    #[test]
    fn words_sit_in_the_middle_of_each_40_m_cell_and_keep_clear_of_ends_and_crossings() {
        let lanes = [lane(LaneKind::Bus, LaneDir::Forward)];
        let d = plan(&lanes, 100.0, &[]);
        assert_eq!(d.len(), 2);
        let mid = |d: &Decal| d.s0 + d.rows() as f32 * d.pitch * 0.5;
        assert!((mid(&d[0]) - 20.0).abs() < 1e-3 && (mid(&d[1]) - 60.0).abs() < 1e-3);
        // A crossing over the first word drops it; a segment under 40 m has none.
        assert_eq!(plan(&lanes, 100.0, &[(15.0, 25.0)]).len(), 1);
        assert!(plan(&lanes, 39.0, &[]).is_empty());
        // Lanes of other kinds get no word.
        assert!(plan(&[lane(LaneKind::Car, LaneDir::Forward)], 100.0, &[]).is_empty());
    }

    #[test]
    fn bus_lights_fifty_pixels() {
        let d = plan(&[lane(LaneKind::Bus, LaneDir::Forward)], 100.0, &[]).remove(0);
        let n: usize = (0..d.rows())
            .map(|j| d.columns(j).iter().filter(|c| c.2).count())
            .sum();
        assert_eq!(n, 50);
    }

    #[test]
    fn trolley_reads_trolley_and_bus_reads_bus() {
        assert_eq!(
            plan(&[lane(LaneKind::Trolley, LaneDir::Forward)], 100.0, &[])[0].text,
            "TROLLEY"
        );
        assert_eq!(
            plan(&[lane(LaneKind::Bus, LaneDir::Forward)], 100.0, &[])[0].text,
            "BUS"
        );
    }

    #[test]
    fn the_first_letter_is_nearest_to_the_driver_in_both_directions() {
        let word = |dir| plan(&[lane(LaneKind::Bus, dir)], 100.0, &[]).remove(0);
        let lit = |d: &Decal, j: usize| d.columns(j).iter().filter(|c| c.2).count();
        // Forward: driving towards +s, the B (nearest) is at the low-s end, its bottom row
        // (4 pixels of "####.") first.
        let f = word(LaneDir::Forward);
        assert_eq!(lit(&f, 0), 4);
        // Backward: the nearest end is the high-s one.
        let b = word(LaneDir::Backward);
        assert_eq!(lit(&b, b.rows() - 1), 4);
        // The gap row between two letters is blank.
        assert_eq!(lit(&f, 7), 0);
    }

    #[test]
    fn every_row_boundary_is_found_again_and_columns_stay_inside_the_lane() {
        let d = plan(&[lane(LaneKind::Trolley, LaneDir::Backward)], 100.0, &[]).remove(0);
        for (j, s) in d.boundaries().enumerate().take(d.rows()) {
            assert_eq!(d.row_at(s), Some(j));
        }
        assert_eq!(d.row_at(d.s0 + d.rows() as f32 * d.pitch), None);
        for c in d.columns(3) {
            assert!(c.0 >= d.offset - d.width * 0.5 && c.1 <= d.offset + d.width * 0.5);
        }
    }
}
