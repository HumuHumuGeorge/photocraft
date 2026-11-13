//! The colour model of the synced HSB + OKLCH picker: sRGB-encoded colours to and from OKLCH
//! (Björn Ottosson's OKLab, implemented from its public specification at
//! <https://bottosson.github.io/posts/oklab/>), the sRGB gamut's chroma limit, and the values the
//! two fields hold for one colour. Pure: no egui here.

use std::f64::consts::TAU;
use std::sync::OnceLock;

use photocraft_color::convert::{hsb_to_rgb, rgb_to_hsb};

/// Chroma at the OKLCH field's right edge. sRGB's most chromatic colour (magenta) has 0.3225.
pub(super) const C_MAX: f32 = 0.33;

/// The OKLCH field's image is never larger than this a side, however large the panel.
pub(super) const FIELD_MAX_PX: usize = 320;

/// Colours down the OKLCH hue strip.
const STRIP_STEPS: usize = 72;

/// Below this chroma a colour has no hue of its own, so the OKLCH field keeps the hue it showed.
const ACHROMATIC: f32 = 1e-4;

/// Below this saturation (or brightness) the HSB field keeps the hue (and the saturation) it
/// showed, as the Hue Cube does.
const GREY: f32 = 0.01;

/// An OKLCH colour: lightness 0..1, chroma (0 is a grey; sRGB ends before [`C_MAX`]) and hue in
/// turns 0..1 (0 along OKLab's +a, counter-clockwise towards +b).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Lch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

/// `x` in `lo..=hi`; `lo` for a NaN or an infinity.
fn sane(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_finite() { x.clamp(lo, hi) } else { lo }
}

/// sRGB transfer function: encoded to linear.
fn decode(v: f64) -> f64 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// sRGB transfer function: linear to encoded, clamped to 0..1.
fn encode(v: f64) -> f64 {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Linear sRGB of OKLab (`l`, `a`, `b`); outside 0..1 when sRGB can't show the colour.
fn linear_of(l: f64, a: f64, b: f64) -> [f64; 3] {
    let cube = |x: f64| x * x * x;
    let l3 = cube(l + 0.396_337_777_4 * a + 0.215_803_757_3 * b);
    let m3 = cube(l - 0.105_561_345_8 * a - 0.063_854_172_8 * b);
    let s3 = cube(l - 0.089_484_177_5 * a - 1.291_485_548_0 * b);
    [
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701_0 * s3,
    ]
}

/// OKLab (`l`, `a`, `b`) of an sRGB-encoded colour.
fn lab_of(rgb: [f32; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|v| decode(f64::from(sane(v, 0.0, 1.0))));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

/// The direction of hue `h` (turns) in OKLab's a/b plane.
fn direction(h: f32) -> (f64, f64) {
    let angle = f64::from(sane(h, 0.0, 1.0)) * TAU;
    (angle.cos(), angle.sin())
}

/// How far under zero a linear channel may be, as a fraction of the largest channel, and still
/// count as sRGB.
///
/// Taken strictly, the gamut is not what a picker needs around pure blue. There the surface of
/// one hue runs almost along a face of the RGB cube, so on the way from grey out to a blue such
/// as (0, 0, 1), at its lightness and hue, red dips to −0.08 % of blue before coming back to 0 at
/// the blue itself (measured over the cube's surface: nowhere else does a channel leave 0..1 on
/// that way, and never above 1). A strict limit stops at the dip, a sixth short of the blue: the
/// purest blues would sit beyond the coloured part of the field, where no pick reaches them. A
/// channel this little under zero shows as zero (the colour moves about 0.003 in OKLab), and
/// counting it as inside moves the limit of no other hue by more than 0.004 of chroma.
const UNDER: f64 = 0.002;

/// Can sRGB show the linear colour `rgb` (see [`UNDER`])?
fn shows(rgb: [f64; 3]) -> bool {
    let [r, g, b] = rgb;
    let (least, most) = (r.min(g).min(b), r.max(g).max(b));
    rgb.iter().all(|v| v.is_finite()) && most <= 1.0 && least >= -UNDER * most
}

/// The most chroma sRGB holds at lightness `l` along the hue direction (`a`, `b`): out from the
/// grey axis the gamut is left once (see [`UNDER`]), so a bisection finds where.
fn limit(l: f64, a: f64, b: f64) -> f64 {
    // Black and white hold none; a NaN lands here too.
    if !(l > 0.0 && l < 1.0) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0_f64, 0.5_f64);
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        if shows(linear_of(l, mid * a, mid * b)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The most chroma sRGB holds at lightness `l` and hue `h`.
pub(super) fn max_chroma(l: f32, h: f32) -> f32 {
    let (a, b) = direction(h);
    limit(f64::from(sane(l, 0.0, 1.0)), a, b) as f32
}

/// The lightness and chroma of sRGB's most chromatic colour at hue `h`: the chroma limit rises
/// with lightness up to it and falls after, so a ternary search finds it.
pub(super) fn cusp(h: f32) -> (f32, f32) {
    let (a, b) = direction(h);
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..40 {
        let third = (hi - lo) / 3.0;
        let (p, q) = (lo + third, hi - third);
        if limit(p, a, b) < limit(q, a, b) {
            lo = p;
        } else {
            hi = q;
        }
    }
    let l = 0.5 * (lo + hi);
    (l as f32, limit(l, a, b) as f32)
}

/// OKLCH of an sRGB-encoded colour. A grey comes back with hue 0.
pub(super) fn rgb_to_lch(rgb: [f32; 3]) -> Lch {
    let [l, a, b] = lab_of(rgb);
    let c = a.hypot(b);
    let h = if c > 0.0 { (b.atan2(a) / TAU).rem_euclid(1.0) } else { 0.0 };
    Lch { l: sane(l as f32, 0.0, 1.0), c: sane(c as f32, 0.0, 1.0), h: sane(h as f32, 0.0, 1.0) }
}

/// The sRGB-encoded colour of `lch`, its chroma cut to what sRGB holds at its lightness and hue.
pub(super) fn lch_to_rgb(lch: Lch) -> [f32; 3] {
    let l = f64::from(sane(lch.l, 0.0, 1.0));
    let (a, b) = direction(lch.h);
    let c = f64::from(sane(lch.c, 0.0, 1.0)).min(limit(l, a, b));
    linear_of(l, c * a, c * b).map(|v| encode(v) as f32)
}

/// The colour `t` of the way from `a` to `b` (sRGB-encoded) along the straight line between them
/// in OKLab. In OKLCH's cylinder that line is a chord: it cuts across inside the arc that turning
/// the hue would follow, so the colours between two hues lose chroma on the way, down to a grey
/// between opposite ones. Where the line leaves sRGB (the gamut is not convex in OKLab) the colour
/// keeps its lightness and hue and takes the chroma sRGB holds. The ends are `a` and `b` exactly.
pub(super) fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = f64::from(sane(t, 0.0, 1.0));
    if t <= 0.0 {
        return a.map(|v| sane(v, 0.0, 1.0));
    }
    if t >= 1.0 {
        return b.map(|v| sane(v, 0.0, 1.0));
    }
    let ([l0, a0, b0], [l1, a1, b1]) = (lab_of(a), lab_of(b));
    let between = |from: f64, to: f64| from + (to - from) * t;
    let (l, x, y) = (between(l0, l1), between(a0, a1), between(b0, b1));
    let c = x.hypot(y);
    // A grey has no direction; any will do for a chroma of zero.
    let (toward_a, toward_b) = if c > 0.0 { (x / c, y / c) } else { (1.0, 0.0) };
    let c = c.min(limit(l, toward_a, toward_b));
    linear_of(l, c * toward_a, c * toward_b).map(|v| encode(v) as f32)
}

/// The OKLCH hue strip from its top (hue 1) to its bottom (hue 0): each hue's most chromatic sRGB
/// colour. The same for every colour, so it is worked out once.
pub(super) fn hue_strip() -> &'static [[f32; 3]] {
    static STRIP: OnceLock<Vec<[f32; 3]>> = OnceLock::new();
    STRIP.get_or_init(|| {
        (0..=STRIP_STEPS)
            .map(|k| {
                let h = 1.0 - k as f32 / STRIP_STEPS as f32;
                let (l, c) = cusp(h);
                lch_to_rgb(Lch { l, c, h })
            })
            .collect()
    })
}

/// The OKLCH field at hue `h` as `width` x `height` straight-alpha RGBA pixels, in rows from the
/// top: lightness 1 down to 0, chroma 0 across to [`C_MAX`]. Outside sRGB is transparent, and the
/// pixel the gamut's edge crosses is as opaque as the part of it inside. Both sides are capped at
/// [`FIELD_MAX_PX`]; the size used is returned with the pixels.
pub(super) fn field_pixels(h: f32, width: usize, height: usize) -> ([usize; 2], Vec<[u8; 4]>) {
    let (width, height) = (width.min(FIELD_MAX_PX), height.min(FIELD_MAX_PX));
    let (a, b) = direction(h);
    let mut pixels = Vec::with_capacity(width * height);
    for j in 0..height {
        let l = 1.0 - (j as f64 + 0.5) / height as f64;
        let top = limit(l, a, b);
        let edge = top / f64::from(C_MAX) * width as f64;
        for i in 0..width {
            let cover = (edge - i as f64).clamp(0.0, 1.0);
            if cover <= 0.0 {
                pixels.push([0; 4]);
                continue;
            }
            let c = ((i as f64 + 0.5) / width as f64 * f64::from(C_MAX)).min(top);
            let [red, green, blue] = linear_of(l, c * a, c * b).map(|v| (encode(v) * 255.0).round() as u8);
            pixels.push([red, green, blue, (cover * 255.0).round() as u8]);
        }
    }
    ([width, height], pixels)
}

/// What the two fields show for one colour. The colour alone can't carry all of it: a grey has no
/// hue, black no saturation either, and a hue's gamut may hold less chroma than was asked for. So
/// the values are kept while they still give the colour, and a colour set elsewhere (the other
/// field, the eyedropper, a swatch) replaces only what it can.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Held {
    /// The colour the values below give.
    pub rgb: [f32; 3],
    /// Hue in turns, saturation and brightness, each 0..1.
    pub hsb: [f32; 3],
    /// The chroma is the one asked for; [`Held::chroma`] is the one the colour has.
    pub lch: Lch,
}

impl Held {
    /// The values of `rgb` itself. A grey has no hue, so its OKLCH hue is that of the HSB hue it
    /// comes with (red's), not whatever the rounding of its near-zero chroma points at.
    pub(super) fn of(rgb: [f32; 3]) -> Held {
        let rgb = rgb.map(|v| sane(v, 0.0, 1.0));
        let [h, s, b] = rgb_to_hsb(rgb);
        let mut lch = rgb_to_lch(rgb);
        if lch.c < ACHROMATIC {
            lch.h = rgb_to_lch(hsb_to_rgb([h, 1.0, 1.0])).h;
        }
        Held { rgb, hsb: [sane(h / 360.0, 0.0, 1.0), sane(s, 0.0, 1.0), sane(b, 0.0, 1.0)], lch }
    }

    /// These values for `rgb`: unchanged while they still give it, otherwise `rgb`'s own with the
    /// hue (and for black the saturation) it can't carry kept from before.
    pub(super) fn follow(self, rgb: [f32; 3]) -> Held {
        if self.rgb.iter().zip(rgb).all(|(a, b)| (a - b).abs() <= 1e-5) {
            return self;
        }
        let mut next = Held::of(rgb);
        let [old_h, old_s, _] = self.hsb;
        let [h, s, b] = next.hsb;
        next.hsb = if b < GREY {
            [old_h, old_s, b]
        } else if s < GREY {
            [old_h, s, b]
        } else {
            [h, s, b]
        };
        if next.lch.c < ACHROMATIC {
            next.lch.h = self.lch.h;
        }
        next
    }

    /// The HSB field or its hue strip picked `hsb` (hue in turns).
    pub(super) fn with_hsb(self, hsb: [f32; 3]) -> Held {
        let hsb = hsb.map(|v| sane(v, 0.0, 1.0));
        let [h, s, b] = hsb;
        let rgb = hsb_to_rgb([h * 360.0, s, b]).map(|v| sane(v, 0.0, 1.0));
        Held { hsb, ..self.follow(rgb) }
    }

    /// The OKLCH field or its hue strip picked `lch`. The chroma asked for is kept even where the
    /// hue's gamut cuts it short, so turning the hue past a narrow part of the gamut and back
    /// returns to the colour it left.
    pub(super) fn with_lch(self, lch: Lch) -> Held {
        let lch = Lch { l: sane(lch.l, 0.0, 1.0), c: sane(lch.c, 0.0, C_MAX), h: sane(lch.h, 0.0, 1.0) };
        Held { lch, ..self.follow(lch_to_rgb(lch)) }
    }

    /// The chroma the colour has: the one asked for, or less where sRGB ends first.
    pub(super) fn chroma(self) -> f32 {
        self.lch.c.min(max_chroma(self.lch.l, self.lch.h))
    }
}
