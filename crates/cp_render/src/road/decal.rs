//! Lane words (T-302d): "BUS" on bus lanes and "TROLLEY" on trolley lanes, every
//! [`SPACING`] metres, as a 5x7 raster of vertex-coloured tiles baked into the lane surface
//! (the lane's own strip is cut out under the word, nothing overlaps). Letters read "up":
//! the first one is the farthest from the driver, so the word reads top to bottom as the driver sees it. Near mesh only.

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
        // Index counted from the far end, as the driver reads the word top to bottom: the
        // first letter is the farthest, each letter's top row is its farthest one.
        let u = if self.forward { self.rows() - 1 - j } else { j };
        let (letter, within) = (u / (ROWS + 1), u % (ROWS + 1));
        let rows = (within < ROWS)
            .then(|| glyph(self.text.as_bytes()[letter]))
            .flatten();
        let pw = self.pixel();
        std::array::from_fn(|i| {
            // Column `c` counts from the driver's left; the driver's left is +x going forward.
            let c = if self.forward { COLS - 1 - i } else { i };
            let lit = rows.is_some_and(|g| g[within].as_bytes()[c] == b'#');
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
            // T-322 (§23.3): a bicycle, its wheels one behind the other along the lane.
            LaneKind::Bike => (BICYCLE, 0.3),
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

/// The "word" of a bike lane: one pictogram ([`glyph`] `b'%'`).
pub const BICYCLE: &str = "%";

/// The 5x7 raster of a letter, top row first; `%` is the bicycle of a bike lane (two wheels
/// and the frame between them, seen from the side and laid along the lane).
fn glyph(c: u8) -> Option<&'static [&'static str; ROWS]> {
    Some(match c {
        b'%' => &[
            ".###.", "#...#", ".###.", "..#..", ".###.", "#...#", ".###.",
        ],
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
    fn the_first_letter_is_the_farthest_from_the_driver_in_both_directions() {
        let word = |dir| plan(&[lane(LaneKind::Bus, dir)], 100.0, &[]).remove(0);
        let lit = |d: &Decal, j: usize| d.columns(j).iter().filter(|c| c.2).count();
        // Forward: driving towards +s, the B (farthest) is at the high-s end, its top row
        // (4 pixels of "####.") first.
        let f = word(LaneDir::Forward);
        assert_eq!(lit(&f, f.rows() - 1), 4);
        // Backward: the farthest end is the low-s one.
        let b = word(LaneDir::Backward);
        assert_eq!(lit(&b, 0), 4);
        // The gap row between two letters is blank; the U below the B starts with 5 pixels.
        assert_eq!(lit(&f, f.rows() - 1 - 7), 0);
        assert_eq!(lit(&f, f.rows() - 1 - 8), 2);
    }

    /// T-322: a bike lane shows a bicycle every 40 m: two wheels of 8 pixels, a frame of 1.
    #[test]
    fn a_bike_lane_shows_a_bicycle_every_40_m() {
        let lanes = [(LaneKind::Bike, LaneDir::Forward, -4.25, 1.5)];
        let d = plan(&lanes, 100.0, &[]);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].text, BICYCLE);
        let n: usize = (0..d[0].rows())
            .map(|j| d[0].columns(j).iter().filter(|c| c.2).count())
            .sum();
        assert_eq!(n, 17);
        for c in d[0].columns(1) {
            assert!(c.0 >= -5.0 && c.1 <= -3.5);
        }
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
