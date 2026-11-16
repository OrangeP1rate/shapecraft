//! Screw-thread standards for the Thread tool: the sizes and pitches each standard offers, and the diameters a
//! thread of a size is modelled at.
//!
//! Sizes and pitches come from the published standards: ISO 261 (ISO metric, plus the miniature and old DIN sizes
//! commonly offered next to it), ASME B1.1 (Unified inch, "ANSI") and ASME B1.5 (Acme). Threads are modelled at the
//! middle of the usual fit's tolerance (ISO 965 medium, 6g bolt / 6H nut, from its formulas; the same formulas are
//! used for Unified and Acme sizes): so a bolt and a nut made here fit, with the standard's own play.
//!
//! On top of that a print clearance can be asked for: an FDM-printed thread swells, so the bolt is made that much
//! smaller and the nut that much larger, all round.

/// A thread standard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Standard {
    Iso,
    Ansi,
    Acme,
}

impl Standard {
    pub const ALL: [Standard; 3] = [Standard::Iso, Standard::Ansi, Standard::Acme];

    pub fn key(self) -> &'static str {
        match self {
            Standard::Iso => "iso",
            Standard::Ansi => "ansi",
            Standard::Acme => "acme",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Standard::Iso => "ISO",
            Standard::Ansi => "ANSI",
            Standard::Acme => "ACME",
        }
    }

    pub fn from_key(s: &str) -> Option<Standard> {
        Standard::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(s) || k.label().eq_ignore_ascii_case(s))
    }

    /// Pitches are given as threads per inch (ANSI, ACME) instead of millimetres (ISO).
    pub fn per_inch(self) -> bool {
        self != Standard::Iso
    }

    /// Half the angle between a thread's flanks, radians.
    pub fn half_angle(self) -> f64 {
        match self {
            Standard::Acme => 14.5_f64.to_radians(),
            _ => 30.0_f64.to_radians(),
        }
    }

    pub fn sizes(self) -> Vec<Size> {
        match self {
            Standard::Iso => ISO.iter().map(|&(d, pitches)| Size { name: iso_name(d), major: d, pitches: pitches.to_vec() }).collect(),
            Standard::Ansi => {
                let mut v: Vec<Size> = UN_NUMBERED
                    .iter()
                    .map(|&(n, tpi)| Size { name: format!("#{n}"), major: (0.060 + 0.013 * f64::from(n)) * INCH, pitches: tpis(tpi) })
                    .collect();
                v.extend(UN.iter().map(|&(sixteenths, tpi)| Size {
                    name: inch_name(sixteenths),
                    major: f64::from(sixteenths) / 16.0 * INCH,
                    pitches: tpis(tpi),
                }));
                // 0.390-27 sits between 3/8" and 7/16" (B1.1's UNS list).
                let at = v.iter().position(|s| s.major > 0.390 * INCH).unwrap_or(v.len());
                v.insert(at, Size { name: "0.390\"".into(), major: 0.390 * INCH, pitches: tpis(&[27.0]) });
                v
            }
            Standard::Acme => ACME
                .iter()
                .map(|&(sixteenths, tpi)| Size {
                    name: inch_name(sixteenths),
                    major: f64::from(sixteenths) / 16.0 * INCH,
                    pitches: tpis(&[tpi]),
                })
                .collect(),
        }
    }

    /// The size whose major diameter is nearest `diameter` (mm).
    pub fn nearest(self, diameter: f64) -> Size {
        let sizes = self.sizes();
        let mut best = 0;
        for (i, s) in sizes.iter().enumerate() {
            if (s.major - diameter).abs() < (sizes[best].major - diameter).abs() {
                best = i;
            }
        }
        sizes.into_iter().nth(best).unwrap_or(Size { name: String::new(), major: diameter, pitches: vec![1.0] })
    }

    /// The size called `name` (as the size list shows it).
    pub fn size(self, name: &str) -> Option<Size> {
        let want = name.trim().replace(' ', "");
        self.sizes().into_iter().find(|s| s.name.replace(' ', "").eq_ignore_ascii_case(&want))
    }

    /// How a pitch shows in the card: "1.5 mm" or "13 tpi".
    pub fn pitch_label(self, pitch: f64) -> String {
        if self.per_inch() {
            let tpi = INCH / pitch;
            let shown = (tpi * 100.0).round() / 100.0;
            format!("{} tpi", trim(shown))
        } else {
            format!("{} mm", trim((pitch * 1000.0).round() / 1000.0))
        }
    }
}

/// One size: what the list calls it, its nominal major diameter and its pitches (mm, coarsest first).
#[derive(Clone, Debug, PartialEq)]
pub struct Size {
    pub name: String,
    pub major: f64,
    pub pitches: Vec<f64>,
}

impl Size {
    /// The coarsest pitch, the one a new thread starts with.
    pub fn default_pitch(&self) -> f64 {
        self.pitches.first().copied().unwrap_or(1.0)
    }

    /// The listed pitch nearest `pitch` (mm).
    pub fn nearest_pitch(&self, pitch: f64) -> f64 {
        self.pitches.iter().copied().min_by(|a, b| (a - pitch).abs().total_cmp(&(b - pitch).abs())).unwrap_or(pitch)
    }
}

pub const INCH: f64 = 25.4;

/// How far (in pitches) a modelled tooth or groove reaches past the thread's base into the body.
pub const OVERLAP: f64 = 0.05;

/// The radii a thread is modelled at (mm), its pitch and flank half-angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub pitch: f64,
    pub half_angle: f64,
    /// Outermost: a bolt's crests, a nut's roots.
    pub major: f64,
    /// Where the tooth (bolt) or the groove (nut) is half the pitch wide.
    pub pitch_radius: f64,
    /// Innermost: a bolt's roots, a nut's crests.
    pub minor: f64,
}

impl Profile {
    /// The width of a bolt's tooth (or a nut's groove) at radius `r`.
    pub fn width_at(&self, r: f64, internal: bool) -> f64 {
        let t = self.half_angle.tan();
        if internal { self.pitch / 2.0 - 2.0 * (r - self.pitch_radius) * t } else { self.pitch / 2.0 + 2.0 * (self.pitch_radius - r) * t }
    }
}

/// The radii of a thread of nominal major diameter `d` and pitch `p` (mm): external (a bolt) or internal (a nut),
/// at the middle of the 6g / 6H tolerance, with `clearance` (mm, radial) taken off a bolt or added to a nut.
pub fn profile(standard: Standard, d: f64, p: f64, internal: bool, clearance: f64) -> Profile {
    let half_angle = standard.half_angle();
    // ISO 965-1 grade 6 tolerances (µm → mm).
    let td = (180.0 * p.powf(2.0 / 3.0) - 3.15 / p.sqrt()) / 1000.0;
    let td2 = 90.0 * p.powf(0.4) * d.max(1.0).powf(0.1) / 1000.0;
    let td1 = if p < 1.0 { 433.0 * p - 190.0 * p.powf(1.22) } else { 230.0 * p.powf(0.7) } / 1000.0;
    let (major, pitch_d, minor) = match (standard, internal) {
        (Standard::Acme, false) => {
            // General-purpose Acme (B1.5 class 2G): a little play at the pitch line, more at the minor diameter.
            let allowance = 0.008 * (d / INCH).sqrt() * INCH;
            let minor_clear = if p >= INCH / 10.0 { 0.010 * INCH } else { 0.005 * INCH };
            (d - 0.05 * p, d - p / 2.0 - allowance - td2 / 2.0, d - p - minor_clear)
        }
        (Standard::Acme, true) => {
            let major_clear = if p >= INCH / 10.0 { 0.020 * INCH } else { 0.010 * INCH };
            (d + major_clear, d - p / 2.0 + 1.32 * td2 / 2.0, d - p + 0.05 * p)
        }
        (_, false) => {
            let es = (15.0 + 11.0 * p) / 1000.0;
            (d - es - td / 2.0, d - 0.649519 * p - es - td2 / 2.0, d - 1.226869 * p - es)
        }
        (_, true) => (d + 0.132 * p, d - 0.649519 * p + 1.32 * td2 / 2.0, d - 1.082532 * p + td1 / 2.0),
    };
    let c = if internal { clearance } else { -clearance };
    Profile { pitch: p, half_angle, major: major / 2.0 + c, pitch_radius: pitch_d / 2.0 + c, minor: minor / 2.0 + c }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn iso_name(d: f64) -> String {
    format!("M{}", trim(d))
}

/// 3/8", 1 1/4", 2".
fn inch_name(sixteenths: u32) -> String {
    let whole = sixteenths / 16;
    let mut num = sixteenths % 16;
    let mut den = 16;
    while num != 0 && num.is_multiple_of(2) {
        num /= 2;
        den /= 2;
    }
    match (whole, num) {
        (w, 0) => format!("{w}\""),
        (0, n) => format!("{n}/{den}\""),
        (w, n) => format!("{w} {n}/{den}\""),
    }
}

fn tpis(tpi: &[f64]) -> Vec<f64> {
    let mut v: Vec<f64> = tpi.to_vec();
    v.sort_by(f64::total_cmp);
    v.into_iter().map(|t| INCH / t).collect()
}

/// ISO metric sizes (mm) and their pitches, coarse first then fine (ISO 261; the miniature ones of ISO 68/DIN 14
/// and the old DIN sizes M1.7, M2.3, M2.6 too). The sizes past M300 use a 6 mm pitch.
const ISO: &[(f64, &[f64])] = &[
    (0.35, &[0.09]),
    (0.4, &[0.1]),
    (0.45, &[0.1]),
    (0.5, &[0.125]),
    (0.55, &[0.125]),
    (0.6, &[0.15]),
    (0.7, &[0.175]),
    (0.8, &[0.2]),
    (0.9, &[0.225]),
    (1.0, &[0.25, 0.2]),
    (1.1, &[0.25, 0.2]),
    (1.2, &[0.25, 0.2]),
    (1.4, &[0.3, 0.2]),
    (1.6, &[0.35, 0.2]),
    (1.7, &[0.35]),
    (1.8, &[0.35, 0.2]),
    (2.0, &[0.4, 0.25]),
    (2.2, &[0.45, 0.25]),
    (2.3, &[0.4]),
    (2.5, &[0.45, 0.35]),
    (2.6, &[0.45]),
    (3.0, &[0.5, 0.35]),
    (3.5, &[0.6, 0.35]),
    (4.0, &[0.7, 0.5]),
    (4.5, &[0.75, 0.5]),
    (5.0, &[0.8, 0.5]),
    (5.5, &[0.5]),
    (6.0, &[1.0, 0.75]),
    (7.0, &[1.0, 0.75]),
    (8.0, &[1.25, 1.0, 0.75]),
    (9.0, &[1.25, 1.0, 0.75]),
    (10.0, &[1.5, 1.25, 1.0, 0.75]),
    (11.0, &[1.5, 1.0, 0.75]),
    (12.0, &[1.75, 1.5, 1.25, 1.0]),
    (14.0, &[2.0, 1.5, 1.25, 1.0]),
    (15.0, &[1.5, 1.0]),
    (16.0, &[2.0, 1.5, 1.0]),
    (17.0, &[1.5, 1.0]),
    (18.0, &[2.5, 2.0, 1.5, 1.0]),
    (20.0, &[2.5, 2.0, 1.5, 1.0]),
    (22.0, &[2.5, 2.0, 1.5, 1.0]),
    (24.0, &[3.0, 2.0, 1.5, 1.0]),
    (25.0, &[2.0, 1.5, 1.0]),
    (26.0, &[1.5]),
    (27.0, &[3.0, 2.0, 1.5, 1.0]),
    (28.0, &[2.0, 1.5, 1.0]),
    (30.0, &[3.5, 3.0, 2.0, 1.5, 1.0]),
    (32.0, &[2.0, 1.5]),
    (33.0, &[3.5, 3.0, 2.0, 1.5]),
    (35.0, &[1.5]),
    (36.0, &[4.0, 3.0, 2.0, 1.5]),
    (38.0, &[1.5]),
    (39.0, &[4.0, 3.0, 2.0, 1.5]),
    (40.0, &[3.0, 2.0, 1.5]),
    (42.0, &[4.5, 4.0, 3.0, 2.0, 1.5]),
    (45.0, &[4.5, 4.0, 3.0, 2.0, 1.5]),
    (48.0, &[5.0, 4.0, 3.0, 2.0, 1.5]),
    (50.0, &[3.0, 2.0, 1.5]),
    (52.0, &[5.0, 4.0, 3.0, 2.0, 1.5]),
    (55.0, &[4.0, 3.0, 2.0, 1.5]),
    (56.0, &[5.5, 4.0, 3.0, 2.0, 1.5]),
    (58.0, &[4.0, 3.0, 2.0, 1.5]),
    (60.0, &[5.5, 4.0, 3.0, 2.0, 1.5]),
    (62.0, &[4.0, 3.0, 2.0, 1.5]),
    (63.0, &[1.5]),
    (64.0, &[6.0, 4.0, 3.0, 2.0, 1.5]),
    (65.0, &[4.0, 3.0, 2.0, 1.5]),
    (68.0, &[6.0, 4.0, 3.0, 2.0, 1.5]),
    (70.0, &[6.0, 4.0, 3.0, 2.0, 1.5]),
    (72.0, &[6.0, 4.0, 3.0, 2.0, 1.5]),
    (75.0, &[4.0, 3.0, 2.0, 1.5]),
    (76.0, &[6.0, 4.0, 3.0, 2.0, 1.5]),
    (78.0, &[2.0]),
    (80.0, &[6.0, 4.0, 3.0, 2.0, 1.5]),
    (82.0, &[2.0]),
    (85.0, &[6.0, 4.0, 3.0, 2.0]),
    (90.0, &[6.0, 4.0, 3.0, 2.0]),
    (95.0, &[6.0, 4.0, 3.0, 2.0]),
    (100.0, &[6.0, 4.0, 3.0, 2.0]),
    (105.0, &[6.0, 4.0, 3.0, 2.0]),
    (110.0, &[6.0, 4.0, 3.0, 2.0]),
    (115.0, &[6.0, 4.0, 3.0, 2.0]),
    (120.0, &[6.0, 4.0, 3.0, 2.0]),
    (125.0, &[6.0, 4.0, 3.0, 2.0]),
    (130.0, &[6.0, 4.0, 3.0, 2.0]),
    (135.0, &[6.0, 4.0, 3.0, 2.0]),
    (140.0, &[6.0, 4.0, 3.0, 2.0]),
    (145.0, &[6.0, 4.0, 3.0, 2.0]),
    (150.0, &[6.0, 4.0, 3.0, 2.0]),
    (155.0, &[6.0, 4.0, 3.0]),
    (160.0, &[6.0, 4.0, 3.0]),
    (165.0, &[6.0, 4.0, 3.0]),
    (170.0, &[6.0, 4.0, 3.0]),
    (175.0, &[6.0, 4.0, 3.0]),
    (180.0, &[6.0, 4.0, 3.0]),
    (185.0, &[6.0, 4.0, 3.0]),
    (190.0, &[6.0, 4.0, 3.0]),
    (195.0, &[6.0, 4.0, 3.0]),
    (200.0, &[6.0, 4.0, 3.0]),
    (205.0, &[6.0, 4.0, 3.0]),
    (210.0, &[6.0, 4.0, 3.0]),
    (215.0, &[6.0, 4.0, 3.0]),
    (220.0, &[6.0, 4.0, 3.0]),
    (225.0, &[6.0, 4.0, 3.0]),
    (230.0, &[6.0, 4.0, 3.0]),
    (235.0, &[6.0, 4.0, 3.0]),
    (240.0, &[6.0, 4.0, 3.0]),
    (245.0, &[6.0, 4.0, 3.0]),
    (250.0, &[6.0, 4.0, 3.0]),
    (255.0, &[6.0, 4.0]),
    (260.0, &[6.0, 4.0]),
    (265.0, &[6.0, 4.0]),
    (270.0, &[6.0, 4.0]),
    (275.0, &[6.0, 4.0]),
    (280.0, &[6.0, 4.0]),
    (285.0, &[6.0, 4.0]),
    (290.0, &[6.0, 4.0]),
    (295.0, &[6.0, 4.0]),
    (300.0, &[6.0, 4.0]),
    (310.0, &[6.0]),
    (320.0, &[6.0]),
    (330.0, &[6.0]),
    (340.0, &[6.0]),
    (350.0, &[6.0]),
    (360.0, &[6.0]),
    (370.0, &[6.0]),
    (380.0, &[6.0]),
    (390.0, &[6.0]),
    (400.0, &[6.0]),
    (410.0, &[6.0]),
    (420.0, &[6.0]),
    (430.0, &[6.0]),
    (440.0, &[6.0]),
    (450.0, &[6.0]),
    (460.0, &[6.0]),
    (470.0, &[6.0]),
    (480.0, &[6.0]),
    (490.0, &[6.0]),
    (500.0, &[6.0]),
    (510.0, &[6.0]),
    (520.0, &[6.0]),
    (530.0, &[6.0]),
    (540.0, &[6.0]),
    (550.0, &[6.0]),
    (560.0, &[6.0]),
    (570.0, &[6.0]),
    (580.0, &[6.0]),
    (590.0, &[6.0]),
    (600.0, &[6.0]),
];

/// Unified numbered sizes (#0-#12) and their threads per inch (UNC, UNF, UNEF and the 32/36 extras).
const UN_NUMBERED: &[(u32, &[f64])] = &[
    (0, &[80.0]),
    (1, &[64.0, 72.0]),
    (2, &[56.0, 64.0]),
    (3, &[48.0, 56.0]),
    (4, &[36.0, 40.0, 48.0]),
    (5, &[40.0, 44.0]),
    (6, &[32.0, 40.0]),
    (8, &[32.0, 36.0]),
    (10, &[24.0, 32.0]),
    (12, &[24.0, 28.0, 32.0]),
];

/// Unified fractional sizes (in sixteenths of an inch) and their threads per inch.
const UN: &[(u32, &[f64])] = &[
    (4, &[20.0, 28.0, 32.0]),
    (5, &[18.0, 24.0, 32.0]),
    (6, &[16.0, 24.0, 32.0]),
    (7, &[14.0, 20.0, 28.0]),
    (8, &[12.0, 13.0, 14.0, 16.0, 18.0, 20.0, 24.0, 27.0, 28.0, 32.0]),
    (9, &[12.0, 18.0, 24.0]),
    (10, &[11.0, 12.0, 16.0, 18.0, 24.0]),
    (11, &[12.0, 16.0, 24.0]),
    (12, &[10.0, 12.0, 16.0, 20.0]),
    (13, &[12.0, 16.0, 20.0]),
    (14, &[9.0, 12.0, 14.0, 16.0, 20.0]),
    (15, &[12.0, 16.0, 20.0]),
    (16, &[8.0, 12.0, 14.0, 16.0, 20.0]),
    (17, &[8.0, 12.0, 16.0, 18.0]),
    (18, &[7.0, 8.0, 12.0, 16.0, 18.0]),
    (19, &[8.0, 12.0, 16.0, 18.0]),
    (20, &[7.0, 8.0, 12.0, 16.0, 18.0]),
    (21, &[8.0, 12.0, 16.0, 18.0]),
    (22, &[6.0, 8.0, 12.0, 16.0, 18.0]),
    (23, &[8.0, 12.0, 16.0, 18.0]),
    (24, &[6.0, 8.0, 12.0, 16.0, 18.0]),
    (25, &[8.0, 12.0, 16.0, 18.0]),
    (26, &[8.0, 12.0, 16.0, 18.0]),
    (27, &[8.0, 12.0, 16.0, 18.0]),
    (28, &[5.0, 8.0, 12.0, 16.0]),
    (29, &[8.0, 12.0, 16.0]),
    (30, &[8.0, 12.0, 16.0]),
    (31, &[8.0, 12.0, 16.0]),
    (32, &[4.5, 8.0, 12.0, 16.0]),
    (33, &[8.0, 12.0, 16.0]),
    (34, &[8.0, 12.0, 16.0]),
    (35, &[8.0, 12.0, 16.0]),
    (36, &[4.5, 8.0, 12.0, 16.0]),
    (37, &[8.0, 12.0, 16.0]),
    (38, &[8.0, 12.0, 16.0]),
    (39, &[8.0, 12.0, 16.0]),
    (40, &[4.0, 8.0, 12.0, 16.0]),
    (42, &[8.0, 12.0, 16.0]),
    (44, &[4.0, 8.0, 12.0, 16.0]),
    (46, &[8.0, 12.0, 16.0]),
    (48, &[4.0, 8.0, 12.0, 16.0]),
    (50, &[4.0, 8.0, 12.0, 16.0]),
    (52, &[4.0, 8.0, 12.0, 16.0]),
    (54, &[4.0, 8.0, 12.0, 16.0]),
    (56, &[4.0, 8.0, 12.0, 16.0]),
    (58, &[4.0, 8.0, 12.0, 16.0]),
    (60, &[4.0, 8.0, 12.0, 16.0]),
    (62, &[4.0, 8.0, 12.0, 16.0]),
    (64, &[4.0, 8.0, 12.0, 16.0]),
];

/// Acme general-purpose sizes (sixteenths of an inch) and their threads per inch (ASME B1.5).
const ACME: &[(u32, f64)] = &[
    (4, 16.0),
    (5, 14.0),
    (6, 12.0),
    (7, 12.0),
    (8, 10.0),
    (10, 8.0),
    (12, 6.0),
    (14, 6.0),
    (16, 5.0),
    (18, 5.0),
    (20, 5.0),
    (22, 4.0),
    (24, 4.0),
    (28, 4.0),
    (32, 4.0),
    (36, 3.0),
    (40, 3.0),
    (44, 3.0),
    (48, 2.0),
    (56, 2.0),
    (64, 2.0),
    (72, 2.0),
    (80, 2.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_the_lists() {
        let iso = Standard::Iso.sizes();
        assert_eq!(iso.first().map(|s| s.name.as_str()), Some("M0.35"));
        assert_eq!(iso.last().map(|s| s.name.as_str()), Some("M600"));
        let m10 = Standard::Iso.size("M10").expect("M10");
        assert_eq!(m10.pitches, vec![1.5, 1.25, 1.0, 0.75]);
        let ansi: Vec<String> = Standard::Ansi.sizes().into_iter().map(|s| s.name).collect();
        assert_eq!(&ansi[..3], ["#0", "#1", "#2"]);
        for want in ["1/4\"", "3/8\"", "0.390\"", "7/16\"", "1/2\"", "1 1/4\"", "2 1/16\"", "4\""] {
            assert!(ansi.iter().any(|n| n == want), "{want} missing from {ansi:?}");
        }
        let half = Standard::Ansi.size("1/2\"").expect("1/2");
        assert_eq!(Standard::Ansi.pitch_label(half.default_pitch()), "12 tpi");
        let acme: Vec<String> = Standard::Acme.sizes().into_iter().map(|s| s.name).collect();
        assert_eq!(acme.first().map(String::as_str), Some("1/4\""));
        assert_eq!(acme.last().map(String::as_str), Some("5\""));
        assert_eq!(Standard::Acme.pitch_label(Standard::Acme.size("1/2\"").expect("acme").default_pitch()), "10 tpi");
        assert_eq!(Standard::Iso.pitch_label(1.25), "1.25 mm");
    }

    #[test]
    fn nearest_size_follows_the_face() {
        assert_eq!(Standard::Iso.nearest(10.0).name, "M10");
        assert_eq!(Standard::Iso.nearest(9.6).name, "M10");
        assert_eq!(Standard::Ansi.nearest(10.0).name, "0.390\"");
        assert_eq!(Standard::Acme.nearest(10.4).name, "7/16\"");
    }

    #[test]
    fn m10_matches_the_6g_and_6h_middles() {
        let bolt = profile(Standard::Iso, 10.0, 1.5, false, 0.0);
        assert!((bolt.major * 2.0 - 9.85).abs() < 0.01, "{bolt:?}");
        assert!((bolt.pitch_radius * 2.0 - 8.928).abs() < 0.01, "{bolt:?}");
        assert!((bolt.minor * 2.0 - 8.14).abs() < 0.03, "{bolt:?}");
        let nut = profile(Standard::Iso, 10.0, 1.5, true, 0.0);
        assert!((nut.minor * 2.0 - 8.526).abs() < 0.01, "{nut:?}");
        assert!((nut.major * 2.0 - 10.198).abs() < 0.01, "{nut:?}");
        // The bolt's crest and the nut's root both have a flat; each tooth is narrower than a pitch at its base.
        assert!(bolt.width_at(bolt.major, false) > 0.05 && bolt.width_at(bolt.minor, false) < 1.5);
        assert!(nut.width_at(nut.major, true) > 0.05 && nut.width_at(nut.minor, true) < 1.5);
    }

    #[test]
    fn print_clearance_opens_both_sides() {
        let (b, n) = (profile(Standard::Iso, 10.0, 1.5, false, 0.2), profile(Standard::Iso, 10.0, 1.5, true, 0.2));
        assert!((profile(Standard::Iso, 10.0, 1.5, false, 0.0).pitch_radius - b.pitch_radius - 0.2).abs() < 1e-9);
        assert!((n.pitch_radius - profile(Standard::Iso, 10.0, 1.5, true, 0.0).pitch_radius - 0.2).abs() < 1e-9);
    }

    #[test]
    fn every_size_makes_a_buildable_profile() {
        for st in Standard::ALL {
            for s in st.sizes() {
                for &p in &s.pitches {
                    for internal in [false, true] {
                        let pr = profile(st, s.major, p, internal, 0.0);
                        assert!(pr.minor > 0.0 && pr.minor < pr.pitch_radius && pr.pitch_radius < pr.major, "{} {p} {pr:?}", s.name);
                        let crest = pr.width_at(pr.major, internal);
                        let base = pr.width_at(pr.minor - OVERLAP * p, internal);
                        assert!(crest > 0.0, "{} {p}: no flat at the crest ({crest})", s.name);
                        assert!(base < p, "{} {p}: turns overlap ({base})", s.name);
                    }
                }
            }
        }
    }
}
