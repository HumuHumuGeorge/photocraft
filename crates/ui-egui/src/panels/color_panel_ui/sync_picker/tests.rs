//! The synced HSB + OKLCH picker: its colour model, and the picker in the Color tab.

use egui::{Modifiers, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use photocraft_color::convert::{hsb_to_rgb, rgb_to_hsb};

use super::model::{C_MAX, FIELD_MAX_PX, Held, Lch, cusp, field_pixels, hue_strip, lch_to_rgb, max_chroma, mix, rgb_to_lch};
use super::{Ends, Opt, Picked, Rows, bar_of, ends_id, picked_id, slots};
use crate::PhotocraftApp;
use crate::dock::Group;
use crate::state::ColorPanelMode;
use crate::theme::{ThemeKind, Tokens};

fn worst(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// OKLCH of the sRGB primaries, white and black, against the values the OKLab specification's
/// matrices give (lightness, chroma, hue in degrees).
#[test]
fn oklch_matches_the_specification() {
    let known = [
        ([1.0, 0.0, 0.0], (0.6280, 0.2577, 29.23)),
        ([0.0, 1.0, 0.0], (0.8664, 0.2948, 142.50)),
        ([0.0, 0.0, 1.0], (0.4520, 0.3132, 264.05)),
        ([1.0, 0.0, 1.0], (0.7017, 0.3225, 328.36)),
        ([1.0, 1.0, 0.0], (0.9680, 0.2110, 109.77)),
    ];
    for (rgb, (l, c, degrees)) in known {
        let q = rgb_to_lch(rgb);
        assert!((q.l - l).abs() < 1e-3 && (q.c - c).abs() < 1e-3 && (q.h * 360.0 - degrees).abs() < 0.05, "{rgb:?}: {q:?}");
    }
    let white = rgb_to_lch([1.0; 3]);
    assert!((white.l - 1.0).abs() < 1e-4 && white.c < 1e-4, "{white:?}");
    assert_eq!(rgb_to_lch([0.0; 3]), Lch { l: 0.0, c: 0.0, h: 0.0 });
}

/// Every sRGB colour is within its own hue's chroma limit, so it comes back from OKLCH as itself
/// and its marker is on the coloured part of the field. The pure blues are the hard ones: taken
/// strictly, sRGB is left on the way out to them, a sixth of their chroma short (this panicked
/// nothing, but gave (0, 0.19, 0.90) for (0, 0, 1) before the limit allowed for it).
#[test]
fn every_srgb_colour_is_within_its_limit() {
    let steps = 16;
    let lattice = (0..=steps).flat_map(|r| (0..=steps).flat_map(move |g| (0..=steps).map(move |b| [r, g, b].map(|v| v as f32 / steps as f32))));
    // The blue edge of the cube and the faces beside it, down to black.
    let blues = (1..=20).flat_map(|k| {
        let b = k as f32 / 20.0;
        [[0.0, 0.0, b], [b * 0.02, 0.0, b], [0.0, b * 0.02, b], [b * 0.1, 0.0, b], [0.0, b * 0.1, b]]
    });
    for rgb in lattice.chain(blues) {
        let q = rgb_to_lch(rgb);
        let back = lch_to_rgb(q);
        assert!(worst(back, rgb) < 1e-3, "{rgb:?} -> {q:?} -> {back:?}");
        assert!(max_chroma(q.l, q.h) >= q.c - 1e-4, "{rgb:?}: chroma {} is past the limit {}", q.c, max_chroma(q.l, q.h));
        assert!((Held::of(rgb).chroma() - q.c).abs() < 1e-4, "{rgb:?}: the marker is not at its chroma");
    }
    // The limit at blue's lightness and hue reaches the blue, and not far past it.
    let blue = rgb_to_lch([0.0, 0.0, 1.0]);
    let edge = max_chroma(blue.l, blue.h);
    assert!(edge >= blue.c - 1e-4 && edge < blue.c + 0.005, "blue has chroma {}, the limit there is {edge}", blue.c);
}

/// The chroma limit is where sRGB ends: a colour at it is itself, and one past it is cut to it.
#[test]
fn max_chroma_is_the_gamut_edge() {
    for h in [0.0, 0.08, 0.3, 0.4, 0.55, 0.73, 0.91] {
        for l in [0.1, 0.35, 0.6, 0.85, 0.97] {
            let edge = max_chroma(l, h);
            assert!(edge > 0.0 && edge < C_MAX, "l {l} h {h}: {edge}");
            let on = lch_to_rgb(Lch { l, c: edge, h });
            let q = rgb_to_lch(on);
            // Near blue the limit counts a channel a hair under zero as inside, and showing it as
            // zero moves the colour that little.
            assert!((q.c - edge).abs() < 5e-3 && (q.l - l).abs() < 5e-3, "l {l} h {h}: the edge colour is {q:?}, not chroma {edge}");
            assert!(worst(lch_to_rgb(Lch { l, c: edge + 0.05, h }), on) < 1e-4, "l {l} h {h}: past the edge");
            // On the edge one channel is at an end of its range.
            assert!(on.iter().any(|v| *v < 2e-3 || *v > 1.0 - 2e-3), "l {l} h {h}: {on:?} is inside the gamut");
        }
    }
    for h in [0.0, 0.5] {
        assert_eq!((max_chroma(0.0, h), max_chroma(1.0, h)), (0.0, 0.0), "black and white hold no chroma");
    }
}

/// The OKLCH field's chroma axis reaches every sRGB colour, with little to spare.
#[test]
fn the_field_spans_srgb() {
    let most = (0..720).map(|k| cusp(k as f32 / 720.0).1).fold(0.0, f32::max);
    assert!(most < C_MAX && most > C_MAX - 0.02, "the most chromatic sRGB colour has {most}");
    // The cusp is the chroma limit's peak over lightness.
    for h in [0.08, 0.3, 0.73] {
        let (l, c) = cusp(h);
        for other in [l - 0.05, l + 0.05] {
            assert!(max_chroma(other, h) < c, "hue {h}: lightness {other} holds more than the cusp at {l}");
        }
    }
    assert_eq!(hue_strip().len(), 73);
    assert!(worst(*hue_strip().first().unwrap(), *hue_strip().last().unwrap()) < 1e-3, "the strip ends on the hue it starts on");
}

/// Never crash: NaN, infinities and absurd sizes give colours, not panics.
#[test]
fn hostile_numbers_give_colours() {
    let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e30];
    let ok = |rgb: [f32; 3]| rgb.iter().all(|v| (0.0..=1.0).contains(v));
    for a in bad {
        for b in bad {
            assert!(ok(lch_to_rgb(Lch { l: a, c: b, h: a })) && ok(lch_to_rgb(Lch { l: 0.5, c: a, h: b })), "{a} {b}");
            assert!(max_chroma(a, b).is_finite() && cusp(a).0.is_finite() && cusp(a).1.is_finite());
            let q = rgb_to_lch([a, b, 0.5]);
            assert!(q.l.is_finite() && q.c.is_finite() && q.h.is_finite(), "{a} {b}: {q:?}");
            let held = Held::of([a, b, a]).follow([b, a, b]).with_hsb([a, b, a]).with_lch(Lch { l: b, c: a, h: b });
            assert!(ok(held.rgb) && ok(held.hsb) && held.chroma().is_finite() && held.lch.h.is_finite(), "{a} {b}: {held:?}");
        }
        assert_eq!(field_pixels(a, 4, 3).1.len(), 12);
        for b in bad {
            assert!(ok(mix([a, b, 0.5], [b, a, a], a)) && ok(mix([0.2, 0.4, 0.6], [0.9, 0.1, 0.3], b)), "{a} {b}");
        }
    }
    assert_eq!(field_pixels(0.3, 0, 0), ([0, 0], Vec::new()));
    let (size, pixels) = field_pixels(0.3, usize::MAX, 1_000_000);
    assert_eq!((size, pixels.len()), ([FIELD_MAX_PX; 2], FIELD_MAX_PX * FIELD_MAX_PX));
    // A degenerate layout still has rectangles to draw into.
    for area in
        [Rect::from_min_size(Pos2::ZERO, Vec2::ZERO), Rect::from_min_size(Pos2::ZERO, vec2(5.0, 5.0)), Rect::from_min_size(Pos2::ZERO, vec2(30.0, 400.0))]
    {
        for rows in [Rows::default(), Rows { sliders: true, gradient: true }] {
            let s = slots(area, rows);
            assert!(s.hsb.width() >= 1.0 && s.lch.height() >= 1.0, "{area:?}: {s:?}");
            assert_eq!((s.sliders.is_some(), s.gradient.is_some()), (rows.sliders, rows.gradient));
        }
    }
}

/// The OKLCH field's image is the gamut's slice: colour from the grey axis out to the edge,
/// nothing past it, and a partly covered pixel where the edge falls.
#[test]
fn the_field_image_follows_the_gamut() {
    let h = 0.73; // blue
    let (w, rows) = (64, 64);
    let (size, pixels) = field_pixels(h, w, rows);
    assert_eq!(size, [w, rows]);
    let mut soft = 0;
    for (j, row) in pixels.chunks(w).enumerate() {
        let l = 1.0 - (j as f32 + 0.5) / rows as f32;
        let edge = max_chroma(l, h) / C_MAX * w as f32;
        for (i, px) in row.iter().enumerate() {
            let [.., alpha] = *px;
            if (i as f32) + 1.0 < edge - 0.01 {
                assert_eq!(alpha, 255, "row {j} column {i}: inside (edge {edge})");
            } else if (i as f32) > edge + 0.01 {
                assert_eq!(*px, [0; 4], "row {j} column {i}: outside (edge {edge})");
            } else if alpha > 0 && alpha < 255 {
                soft += 1;
            }
        }
        // The grey axis is the grey of that lightness.
        if edge > 1.5 {
            let [r, g, b, _] = row[0];
            assert!(r.abs_diff(g) <= 4 && g.abs_diff(b) <= 12, "row {j}: {:?} is no grey", row[0]);
        }
    }
    assert!(soft > rows / 2, "only {soft} edge pixels are partly covered");
}

/// A colour set elsewhere moves both fields, but a grey keeps the hues they showed and black the
/// HSB saturation too; the same colour again changes nothing.
#[test]
fn a_grey_keeps_the_hue_and_black_the_saturation() {
    let orange = Held::of([0.9, 0.5, 0.1]);
    assert!(orange.lch.c > 0.1 && orange.hsb[1] > 0.5);
    assert_eq!(orange.follow(orange.rgb), orange);
    let grey = orange.follow([0.5, 0.5, 0.5]);
    assert_eq!((grey.hsb[0], grey.lch.h), (orange.hsb[0], orange.lch.h), "the hues stay");
    assert!(grey.hsb[1] < 1e-6 && grey.lch.c < 1e-4 && (grey.hsb[2] - 0.5).abs() < 1e-6);
    let black = orange.follow([0.0; 3]);
    assert_eq!(black.hsb, [orange.hsb[0], orange.hsb[1], 0.0], "black keeps the hue and the saturation");
    assert_eq!(black.lch.h, orange.lch.h);
    let teal = grey.follow([0.1, 0.6, 0.6]);
    assert!((teal.hsb[0] - 0.5).abs() < 1e-4 && (teal.lch.h - rgb_to_lch(teal.rgb).h).abs() < 1e-6, "a colour brings its own hues: {teal:?}");
    // A grey with nothing shown before it: both fields show red's hue, not a rounding error's.
    for grey in [[0.5; 3], [1.0; 3], [0.0; 3], [0.2; 3]] {
        let first = Held::of(grey);
        assert_eq!((first.hsb[0], first.lch.h), (0.0, rgb_to_lch([1.0, 0.0, 0.0]).h), "{grey:?}: {first:?}");
    }
}

/// A pick in one model gives the colour and the other model's values of it; what the colour
/// can't carry stays where it was picked.
#[test]
fn a_pick_in_one_model_moves_the_other() {
    let start = Held::of([0.2, 0.6, 0.8]);
    let picked = start.with_hsb([0.1, 0.8, 0.9]);
    assert!(worst(picked.rgb, hsb_to_rgb([36.0, 0.8, 0.9])) < 1e-6);
    assert_eq!(picked.lch, rgb_to_lch(picked.rgb));
    let q = Lch { l: 0.7, c: 0.1, h: 0.4 };
    let picked = picked.with_lch(q);
    assert_eq!(picked.lch, q);
    assert!(worst(picked.rgb, lch_to_rgb(q)) < 1e-6);
    let [h, s, b] = rgb_to_hsb(picked.rgb);
    assert!(worst(picked.hsb, [h / 360.0, s, b]) < 1e-6);
    // Along the bottom of the HSB field every colour is black: the marker still follows.
    let bottom = picked.with_hsb([0.3, 0.25, 0.0]).with_hsb([0.3, 0.75, 0.0]);
    assert_eq!((bottom.rgb, bottom.hsb), ([0.0; 3], [0.3, 0.75, 0.0]));
    // Top of the hue strips: hue 1 is hue 0's colour, and stays at the top.
    assert_eq!(picked.with_hsb([1.0, 1.0, 1.0]).hsb[0], 1.0);
    assert!(worst(picked.with_hsb([1.0, 1.0, 1.0]).rgb, [1.0, 0.0, 0.0]) < 1e-6);
}

/// Turning the OKLCH hue through a part of the gamut that holds less chroma cuts the colour's
/// chroma there, but not the chroma asked for: back at the first hue the colour is back.
#[test]
fn the_chroma_asked_for_survives_a_narrow_hue() {
    let magenta = Held::of([0.5; 3]).with_lch(Lch { l: 0.7, c: 0.3, h: 0.912 });
    assert!((magenta.chroma() - 0.3).abs() < 1e-6, "sRGB holds it: {}", magenta.chroma());
    let cyan = magenta.with_lch(Lch { h: 0.54, ..magenta.lch });
    assert!(cyan.chroma() < 0.15 && cyan.lch.c == 0.3, "{cyan:?}");
    assert!((rgb_to_lch(cyan.rgb).c - cyan.chroma()).abs() < 1e-3, "the colour has the chroma that fits");
    let back = cyan.with_lch(Lch { h: 0.912, ..cyan.lch });
    assert!(worst(back.rgb, magenta.rgb) < 1e-6);
}

/// The gradient between two colours is the straight line between them in OKLab: every point of it
/// lies that far along in lightness, a and b alike, and its ends are the two colours themselves.
#[test]
fn the_gradient_is_the_straight_line_through_oklab() {
    let lab = |rgb: [f32; 3]| {
        let q = rgb_to_lch(rgb);
        let angle = q.h * std::f32::consts::TAU;
        [q.l, q.c * angle.cos(), q.c * angle.sin()]
    };
    let (a, b) = ([0.8, 0.3, 0.2], [0.2, 0.4, 0.8]);
    assert_eq!((mix(a, b, 0.0), mix(a, b, 1.0)), (a, b));
    assert_eq!((mix(a, b, -3.0), mix(a, b, 7.0)), (a, b), "past its ends it is the end");
    let (from, to) = (lab(a), lab(b));
    for k in 1..16 {
        let t = k as f32 / 16.0;
        let got = lab(mix(a, b, t));
        for i in 0..3 {
            let want = from[i] + (to[i] - from[i]) * t;
            assert!((got[i] - want).abs() < 2e-3, "at {t}: OKLab {got:?}, wanted {want} in place {i}");
        }
    }
    // A chord, not an arc: between two colours of opposite hues it passes through a grey.
    let red = rgb_to_lch([1.0, 0.0, 0.0]);
    let opposite = Lch { l: red.l, c: 0.1, h: (red.h + 0.5).fract() };
    let grey = mix([1.0, 0.0, 0.0], lch_to_rgb(opposite), red.c / (red.c + opposite.c));
    assert!(rgb_to_lch(grey).c < 3e-3 && (rgb_to_lch(grey).l - red.l).abs() < 3e-3, "{grey:?}: {:?}", rgb_to_lch(grey));
    // Black to white is the greys, evenly in lightness.
    for k in 0..=10 {
        let q = rgb_to_lch(mix([0.0; 3], [1.0; 3], k as f32 / 10.0));
        assert!(q.c < 1e-3 && (q.l - k as f32 / 10.0).abs() < 1e-3, "step {k}: {q:?}");
    }
    // Between the cube's corners the line may leave sRGB: the colour is still at the line's
    // lightness and hue, with the chroma sRGB holds there.
    let corners = [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0]];
    for a in corners {
        for b in corners {
            let (from, to) = (lab(a), lab(b));
            for k in 1..12 {
                let t = k as f32 / 12.0;
                let want = [0, 1, 2].map(|i| from[i] + (to[i] - from[i]) * t);
                let (got, chroma) = (lab(mix(a, b, t)), want[1].hypot(want[2]));
                assert!((got[0] - want[0]).abs() < 6e-3, "{a:?} to {b:?} at {t}: lightness {} for {}", got[0], want[0]);
                assert!(got[1].hypot(got[2]) <= chroma + 6e-3, "{a:?} to {b:?} at {t}: more chroma than the line has");
                // The same hue, where there is chroma to have one.
                let cross = got[1] * want[2] - got[2] * want[1];
                assert!(chroma < 0.02 || cross.abs() < 0.012 * chroma, "{a:?} to {b:?} at {t}: {got:?} is off the hue of {want:?}");
            }
        }
    }
}

/// The Color tab of `kind` for `app` with the picker on, alone in a panel of `size` whose dock
/// group offers it `fill` points of height.
fn tab(kind: ThemeKind, size: Vec2, fill: f32, app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
    let index = Group::Color.tabs(Tokens::for_kind(kind).pro).iter().position(|name| *name == "Color").unwrap();
    let mut h = Harness::builder().with_size(size).with_step_dt(1.0 / 60.0).build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                ui.data_mut(|d| d.insert_temp(crate::panels::color_field_fill_id(), fill));
                crate::panels::dock_body(app, ui, Group::Color, index);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, kind);
    Opt::Enabled.set(&h.ctx, true);
    h.run_steps(3);
    h
}

fn app_with(foreground: [f32; 3]) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let [r, g, b] = foreground;
    app.session.tools.foreground = [r, g, b, 1.0];
    app
}

fn rect(h: &Harness<'_, PhotocraftApp>, label: &str) -> Rect {
    h.query_by_label(label).unwrap_or_else(|| panic!("no {label}")).rect()
}

/// The point at fractions (`x`, `y`) across and down the part named `label`.
fn point(h: &Harness<'_, PhotocraftApp>, label: &str, x: f32, y: f32) -> Pos2 {
    let r = rect(h, label);
    pos2(r.left() + x * r.width(), r.top() + y * r.height())
}

fn button(h: &mut Harness<'_, PhotocraftApp>, p: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(1);
}

fn press(h: &mut Harness<'_, PhotocraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    button(h, p, true);
    button(h, p, false);
    h.run_steps(2);
}

fn rgb(c: [f32; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]]
}

fn held(h: &Harness<'_, PhotocraftApp>, background: bool) -> Held {
    h.ctx.data(|d| d.get_temp::<Held>(egui::Id::unique(("sync-picker-held", background)))).expect("the picker has drawn")
}

/// Off until its menu row turns it on: the Hue Cube is Photoshop's.
#[test]
fn off_until_asked_for() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 400.0), 360.0, app_with([1.0, 0.0, 0.0]));
    assert!(h.query_by_label("OKLCH Field").is_some() && h.query_by_label("#").is_none(), "on: the two fields, no hex readout");
    Opt::Enabled.set(&h.ctx, false);
    h.run_steps(2);
    assert!(h.query_by_label("OKLCH Field").is_none() && h.query_by_label("#").is_some(), "off: the Hue Cube and its readout");
    assert!(!Opt::Enabled.get(&egui::Context::default()), "a fresh context has it off");
    // Only the Hue Cube gives way to it.
    Opt::Enabled.set(&h.ctx, true);
    for mode in ColorPanelMode::ALL {
        h.state_mut().ui.color_panel.mode = mode;
        h.run_steps(2);
        assert_eq!(h.query_by_label("OKLCH Field").is_some(), mode == ColorPanelMode::HueCube, "{mode:?}");
    }
}

/// The panel menu's row turns the picker on (and shows the Hue Cube mode it sits in) and off; the
/// lines' row is there while it is on.
#[test]
fn the_panel_menu_turns_it_and_its_lines_on_and_off() {
    let mut app = app_with([0.2, 0.6, 0.8]);
    app.ui.color_panel.mode = ColorPanelMode::RgbSliders;
    let mut h = Harness::builder().with_size(vec2(260.0, 700.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                crate::color_panel_ui::panel_menu(app, ui);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, ThemeKind::ProMedium);
    h.run_steps(2);
    assert!(h.query_by_label("   HSB + OKLCH").is_some() && h.query_by_label("✔ OKLCH Lines").is_none(), "off, and no lines row");
    h.get_by_label("   HSB + OKLCH").click();
    h.run_steps(2);
    assert!(super::enabled(&h.ctx));
    assert_eq!(h.state().ui.color_panel.mode, ColorPanelMode::HueCube, "it shows in the Hue Cube's place");
    assert!(h.query_by_label("✔ HSB + OKLCH").is_some() && h.query_by_label("✔ Hue Cube").is_some());
    h.get_by_label("✔ OKLCH Lines").click();
    h.run_steps(2);
    assert!(!Opt::Lines.get(&h.ctx) && h.query_by_label("   OKLCH Lines").is_some());
    // The sliders and the gradient are on with it, each with a row of its own.
    for (part, label) in [(Opt::Sliders, "OKLCH Sliders"), (Opt::Gradient, "OKLab Gradient")] {
        assert!(part.get(&h.ctx), "{label} is on");
        h.get_by_label(&format!("✔ {label}")).click();
        h.run_steps(2);
        assert!(!part.get(&h.ctx) && h.query_by_label(&format!("   {label}")).is_some(), "{label} turned off");
        assert_eq!(Rows::shown(&h.ctx), Rows { sliders: Opt::Sliders.get(&h.ctx), gradient: Opt::Gradient.get(&h.ctx) });
        h.get_by_label(&format!("   {label}")).click();
        h.run_steps(2);
        assert!(part.get(&h.ctx), "{label} turned back on");
    }
    // Another mode: the row is unchecked, and picking it comes back to the picker.
    h.get_by_label("   Lab Sliders").click();
    h.run_steps(2);
    assert!(super::enabled(&h.ctx), "another mode leaves the option alone");
    h.get_by_label("   HSB + OKLCH").click();
    h.run_steps(2);
    assert_eq!((super::enabled(&h.ctx), h.state().ui.color_panel.mode), (true, ColorPanelMode::HueCube));
    h.get_by_label("✔ HSB + OKLCH").click();
    h.run_steps(2);
    assert!(!super::enabled(&h.ctx));
    assert_eq!(h.state().ui.color_panel.mode, ColorPanelMode::HueCube, "back to the plain Hue Cube");
}

/// A press in either field sets the colour that field shows there, and the other field's values
/// follow it.
#[test]
fn a_press_in_either_field_sets_the_colour_and_moves_the_other() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 400.0), 360.0, app_with([0.2, 0.6, 0.8]));
    let before = held(&h, false);
    assert!(worst(before.rgb, [0.2, 0.6, 0.8]) < 1e-6);

    let p = point(&h, "OKLCH Field", 0.25, 0.4);
    press(&mut h, p);
    let want = Lch { l: 0.6, c: 0.25 * C_MAX, h: before.lch.h };
    assert!(max_chroma(want.l, want.h) > want.c, "the press is inside the gamut");
    let fg = rgb(h.state().session.tools.foreground);
    assert!(worst(fg, lch_to_rgb(want)) < 2e-3, "OKLCH press: {fg:?}, wanted {:?}", lch_to_rgb(want));
    let now = held(&h, false);
    let [hue, saturation, brightness] = rgb_to_hsb(fg);
    assert!(worst(now.hsb, [hue / 360.0, saturation, brightness]) < 1e-5, "the HSB field follows: {now:?}");

    let p = point(&h, "HSB Field", 0.75, 0.25);
    press(&mut h, p);
    let fg = rgb(h.state().session.tools.foreground);
    let want = hsb_to_rgb([now.hsb[0] * 360.0, 0.75, 0.75]);
    assert!(worst(fg, want) < 2e-3, "HSB press: {fg:?}, wanted {want:?}");
    let now = held(&h, false);
    assert_eq!(now.lch, rgb_to_lch(fg), "the OKLCH field follows");
    assert_eq!(rgb(h.state().session.tools.background), [1.0; 3], "the background is left alone");
}

/// Past the gamut's edge the OKLCH field picks the edge, and a drag that leaves a field keeps
/// picking along its border.
#[test]
fn the_oklch_field_stops_at_the_gamut_and_drags_clamp() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 400.0), 360.0, app_with([0.2, 0.6, 0.8]));
    let hue = held(&h, false).lch.h;
    let p = point(&h, "OKLCH Field", 0.97, 0.5);
    press(&mut h, p);
    let now = held(&h, false);
    let edge = max_chroma(0.5, hue);
    assert!(edge < 0.9 * C_MAX && (now.lch.c - edge).abs() < 2e-3 && (now.chroma() - edge).abs() < 2e-3, "{now:?}, edge {edge}");

    // Press in the HSB field, then drag far out past its top right corner: full saturation and brightness.
    let (from, to) = (point(&h, "HSB Field", 0.5, 0.5), point(&h, "HSB Field", 3.0, -2.0));
    h.hover_at(from);
    h.run_steps(1);
    button(&mut h, from, true);
    h.hover_at(to);
    h.run_steps(2);
    let dragged = held(&h, false);
    assert_eq!((dragged.hsb[1], dragged.hsb[2]), (1.0, 1.0), "{dragged:?}");
    button(&mut h, to, false);
    let fg = rgb(h.state().session.tools.foreground);
    assert!(worst(fg, hsb_to_rgb([dragged.hsb[0] * 360.0, 1.0, 1.0])) < 1e-6);
}

/// Each hue strip turns its own model's hue and keeps the other two values.
#[test]
fn the_hue_strips_turn_their_own_hue() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 400.0), 360.0, app_with([0.2, 0.6, 0.8]));
    let before = held(&h, false);
    let p = point(&h, "HSB Hue", 0.5, 0.75);
    press(&mut h, p);
    let now = held(&h, false);
    assert!((now.hsb[0] - 0.25).abs() < 5e-3 && (now.hsb[1] - before.hsb[1]).abs() < 1e-5 && (now.hsb[2] - before.hsb[2]).abs() < 1e-5, "{now:?}");
    assert!(worst(rgb(h.state().session.tools.foreground), hsb_to_rgb([now.hsb[0] * 360.0, now.hsb[1], now.hsb[2]])) < 1e-6);

    let before = now;
    let p = point(&h, "OKLCH Hue", 0.5, 0.2);
    press(&mut h, p);
    let now = held(&h, false);
    assert!((now.lch.h - 0.8).abs() < 5e-3 && now.lch.l == before.lch.l && now.lch.c == before.lch.c, "{now:?}");
    let q = rgb_to_lch(rgb(h.state().session.tools.foreground));
    assert!((q.h - now.lch.h).abs() < 2e-3 && (q.l - now.lch.l).abs() < 2e-3 && (q.c - now.chroma()).abs() < 2e-3, "{q:?}");
}

/// With the background chip picked the fields edit the background, each chip with values of its own.
#[test]
fn the_background_chip_edits_the_background() {
    let mut app = app_with([0.2, 0.6, 0.8]);
    app.ui.color_panel.background = true;
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 400.0), 360.0, app);
    assert!(worst(held(&h, true).rgb, [1.0; 3]) < 1e-6, "the fields show the background");
    let p = point(&h, "HSB Field", 0.9, 0.1);
    press(&mut h, p);
    let bg = rgb(h.state().session.tools.background);
    let [hue, saturation, brightness] = held(&h, true).hsb;
    assert!((saturation - 0.9).abs() < 2e-3 && (brightness - 0.9).abs() < 2e-3, "{:?}", held(&h, true));
    assert!(worst(bg, hsb_to_rgb([hue * 360.0, saturation, brightness])) < 1e-6 && worst(bg, [1.0; 3]) > 0.5, "{bg:?}");
    assert!(worst(rgb(h.state().session.tools.foreground), [0.2, 0.6, 0.8]) < 1e-6, "the foreground is left alone");
}

/// The picker draws in every theme with and without its lines, in a wide low panel (side by side)
/// and a narrow tall one (one over the other), and each field keeps a usable size. (The fields
/// alone: the sliders and the gradient are in `the_rows_sit_under_the_fields_in_every_theme`.)
#[test]
fn it_lays_out_in_every_theme_and_shape() {
    for kind in ThemeKind::ALL {
        for (size, fill, side_by_side) in [(vec2(300.0, 400.0), 150.0, true), (vec2(250.0, 600.0), 520.0, false), (vec2(600.0, 400.0), 300.0, true)] {
            let mut h = tab(kind, size, fill, app_with([0.9, 0.5, 0.1]));
            Opt::Sliders.set(&h.ctx, false);
            Opt::Gradient.set(&h.ctx, false);
            h.run_steps(2);
            let (hsb, lch) = (rect(&h, "HSB Field"), rect(&h, "OKLCH Field"));
            assert_eq!(lch.left() > hsb.right(), side_by_side, "{kind:?} {size:?}: {hsb:?} {lch:?}");
            assert_eq!(lch.top() > hsb.bottom(), !side_by_side, "{kind:?} {size:?}: {hsb:?} {lch:?}");
            assert!((hsb.size() - lch.size()).length() < 0.01, "{kind:?} {size:?}: {hsb:?} {lch:?}");
            assert!(hsb.width().min(hsb.height()) >= 80.0, "{kind:?} {size:?}: a field of {:?}", hsb.size());
            for label in ["HSB Hue", "OKLCH Hue"] {
                let strip = rect(&h, label);
                assert!(strip.right() + 7.0 <= size.x && (strip.height() - hsb.height()).abs() < 0.01, "{kind:?} {size:?}: {label} {strip:?}");
            }
            assert!(lch.bottom() + 6.0 + 38.0 <= fill + 12.0, "{kind:?} {size:?}: the foot runs past the {fill} pt offered ({lch:?})");
            Opt::Lines.set(&h.ctx, false);
            h.run_steps(2);
        }
    }
}

/// The fields alone fit the Color group at its default height in the dock, in every theme. With
/// the sliders and the gradient too the group is too low: the fields keep their least height and
/// the panel scrolls.
#[test]
fn it_fits_the_default_color_group() {
    for kind in ThemeKind::ALL {
        let mut app = app_with([0.9, 0.5, 0.1]);
        app.ui.theme = kind;
        app.ui.dock_tabs.color = Group::Color.tabs(Tokens::for_kind(kind).pro).iter().position(|name| *name == "Color").unwrap();
        let mut h = Harness::builder().with_size(vec2(1440.0, 1200.0)).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    crate::panels::dock_area(app, ui);
                    egui::CentralPanel::default().show(ui, |_| {});
                }
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, kind);
        Opt::Enabled.set(&h.ctx, true);
        h.run_steps(4);
        let crowded = rect(&h, "HSB Field");
        assert!((crowded.height() - super::FIELD_MIN_H).abs() < 0.01 && h.query_by_label("OKLCH L").is_some(), "{kind:?}: {crowded:?}");
        Opt::Sliders.set(&h.ctx, false);
        Opt::Gradient.set(&h.ctx, false);
        h.run_steps(4);
        let group = crate::panel_docking::probe::native_group_rect(h.state(), &h.ctx, Group::Color).unwrap();
        let (hsb, lch, strip) = (rect(&h, "HSB Field"), rect(&h, "OKLCH Field"), rect(&h, "OKLCH Hue"));
        assert!(lch.left() > hsb.right(), "{kind:?}: side by side in the default dock");
        assert!(hsb.width().min(hsb.height()) >= 60.0, "{kind:?}: a field of {:?} in {group:?}", hsb.size());
        assert!(strip.right() + 7.0 <= group.right(), "{kind:?}: {strip:?} runs past {group:?}");
        // The group's scroll area ends at least 8 pt above its bottom edge (the card's bottom margin).
        assert!(hsb.bottom() + 6.0 + 38.0 <= group.bottom() - 8.0, "{kind:?}: the foot under {hsb:?} runs past the Color group {group:?}");
    }
}

/// With the sliders and the gradient on, they sit under the fields in that order, over the foot,
/// inside what the dock group offers, and the fields keep a usable size, in every theme.
#[test]
fn the_rows_sit_under_the_fields_in_every_theme() {
    for kind in ThemeKind::ALL {
        for (size, fill) in [(vec2(300.0, 420.0), 330.0_f32), (vec2(250.0, 700.0), 620.0), (vec2(600.0, 420.0), 330.0)] {
            let h = tab(kind, size, fill, app_with([0.9, 0.5, 0.1]));
            let (hsb, lch) = (rect(&h, "HSB Field"), rect(&h, "OKLCH Field"));
            assert!(hsb.width().min(hsb.height()) >= 80.0, "{kind:?} {size:?}: a field of {:?}", hsb.size());
            let mut above = hsb.bottom().max(lch.bottom());
            for label in ["OKLCH L", "OKLCH C", "OKLCH H", "OKLab Gradient"] {
                let row = rect(&h, label);
                assert!(row.top() >= above && row.height() > 15.0 && row.width() > 100.0, "{kind:?} {size:?}: {label} at {row:?}, under {above}");
                assert!(row.right() <= size.x, "{kind:?} {size:?}: {label} at {row:?}");
                above = row.bottom();
            }
            for (label, track) in [("OKLCH L value", "OKLCH L"), ("OKLCH C value", "OKLCH C"), ("OKLCH H value", "OKLCH H")] {
                let (field, track) = (rect(&h, label), rect(&h, track));
                assert!(field.left() >= track.right() && field.right() <= size.x, "{kind:?} {size:?}: {label} at {field:?} beside {track:?}");
                assert!(field.center().y > track.top() && field.center().y < track.bottom(), "{kind:?} {size:?}: {label} at {field:?} beside {track:?}");
            }
            let (start, end, gradient) = (rect(&h, "Gradient Start"), rect(&h, "Gradient End"), rect(&h, "OKLab Gradient"));
            assert!(
                start.right() <= gradient.left() && gradient.right() <= end.left() && end.right() <= size.x,
                "{kind:?} {size:?}: {start:?} {gradient:?} {end:?}"
            );
            assert!(above + 6.0 + 38.0 <= fill + 12.0, "{kind:?} {size:?}: the foot runs past the {fill} pt offered (rows end at {above})");
        }
    }
}

/// The point `x` of the way along the bar of the track named `label`.
fn on_track(h: &Harness<'_, PhotocraftApp>, label: &str, x: f32) -> Pos2 {
    let track = rect(h, label);
    let bar = bar_of(track);
    pos2(bar.left() + x * bar.width(), track.center().y)
}

/// A press on a slider's track sets that channel and leaves the other two; past the gamut's edge
/// the chroma slider gives the edge.
#[test]
fn the_sliders_set_their_own_channel() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 460.0), 420.0, app_with([0.2, 0.6, 0.8]));
    let before = held(&h, false);
    let p = on_track(&h, "OKLCH L", 0.4);
    press(&mut h, p);
    let now = held(&h, false);
    assert!((now.lch.l - 0.4).abs() < 2e-3 && now.lch.c == before.lch.c && now.lch.h == before.lch.h, "{now:?}");
    let fg = rgb(h.state().session.tools.foreground);
    assert!(worst(fg, lch_to_rgb(now.lch)) < 1e-6 && (rgb_to_lch(fg).l - 0.4).abs() < 3e-3, "{fg:?}");

    let p = on_track(&h, "OKLCH H", 0.75);
    press(&mut h, p);
    let turned = held(&h, false);
    assert!((turned.lch.h - 0.75).abs() < 2e-3 && turned.lch.l == now.lch.l && turned.lch.c == now.lch.c, "{turned:?}");
    assert!((rgb_to_lch(rgb(h.state().session.tools.foreground)).h - 0.75).abs() < 3e-3);

    // A quarter of the way along the chroma track is a quarter of the field's width of chroma.
    let p = on_track(&h, "OKLCH C", 0.25);
    press(&mut h, p);
    let muted = held(&h, false);
    assert!(max_chroma(muted.lch.l, muted.lch.h) > 0.25 * C_MAX, "the press is inside the gamut");
    assert!((muted.lch.c - 0.25 * C_MAX).abs() < 2e-3 && muted.lch.l == turned.lch.l && muted.lch.h == turned.lch.h, "{muted:?}");
    assert!((rgb_to_lch(rgb(h.state().session.tools.foreground)).c - 0.25 * C_MAX).abs() < 3e-3);
    let p = on_track(&h, "OKLCH C", 1.0);
    press(&mut h, p);
    let most = held(&h, false);
    let edge = max_chroma(most.lch.l, most.lch.h);
    assert!(edge < 0.95 * C_MAX && (most.lch.c - edge).abs() < 1e-6 && (most.chroma() - edge).abs() < 1e-6, "{most:?}, edge {edge}");

    // A drag that leaves the track keeps to its ends.
    let (from, to) = (on_track(&h, "OKLCH L", 0.5), on_track(&h, "OKLCH L", 4.0) + vec2(0.0, 90.0));
    h.hover_at(from);
    h.run_steps(1);
    button(&mut h, from, true);
    h.hover_at(to);
    h.run_steps(2);
    button(&mut h, to, false);
    assert_eq!(held(&h, false).lch.l, 1.0);
    assert!(worst(rgb(h.state().session.tools.foreground), [1.0; 3]) < 1e-3, "full lightness is white");
}

/// Type `text` into the value field named `label`, replacing what it shows, and leave the field.
fn type_into(h: &mut Harness<'_, PhotocraftApp>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    for ch in text.chars() {
        h.event(egui::Event::Text(ch.to_string()));
        h.run_steps(1);
    }
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
}

/// A number typed into a slider's field sets that channel; one off its track, or no number at
/// all, changes nothing and the field goes back to the colour's value.
#[test]
fn values_typed_into_the_sliders_fields_set_their_channel() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 460.0), 420.0, app_with([0.2, 0.6, 0.8]));
    type_into(&mut h, "OKLCH L value", "0.42");
    type_into(&mut h, "OKLCH H value", "200");
    type_into(&mut h, "OKLCH C value", "0.05");
    let now = held(&h, false);
    assert_eq!((now.lch.l, now.lch.c, now.lch.h), (0.42, 0.05, 200.0 / 360.0));
    let q = rgb_to_lch(rgb(h.state().session.tools.foreground));
    assert!((q.l - 0.42).abs() < 3e-3 && (q.c - 0.05).abs() < 3e-3 && (q.h * 360.0 - 200.0).abs() < 1.0, "{q:?}");
    for (label, bad) in [("OKLCH L value", "7"), ("OKLCH C value", "-0.1"), ("OKLCH H value", "361"), ("OKLCH L value", "light"), ("OKLCH H value", "")] {
        type_into(&mut h, label, bad);
        assert_eq!(held(&h, false), now, "{label}: {bad:?} changes nothing");
    }
    // More chroma than the gamut holds there is the gamut's edge, and the field shows it.
    type_into(&mut h, "OKLCH C value", "0.3");
    let most = held(&h, false);
    let edge = max_chroma(0.42, 200.0 / 360.0);
    assert!(edge < 0.2 && (most.lch.c - edge).abs() < 1e-6, "{most:?}, edge {edge}");
    let shown = h.get_by_label("OKLCH C value").accesskit_node().value().map(|v| v.to_string()).unwrap_or_default();
    assert_eq!(shown, format!("{edge:.3}"));
}

/// The gradient starts between the foreground and the background; a click on an end's chip puts
/// the current colour there, and a press along it picks the colour of the straight line through
/// OKLab between the two, its ends being the two colours themselves.
#[test]
fn the_gradient_takes_its_ends_from_the_colour_and_picks_between_them() {
    let mut h = tab(ThemeKind::ProMedium, vec2(300.0, 460.0), 420.0, app_with([0.9, 0.5, 0.1]));
    let ends = |h: &Harness<'_, PhotocraftApp>| h.ctx.data_mut(|d| d.get_persisted::<Ends>(ends_id())).expect("the gradient has drawn");
    let picked = |h: &Harness<'_, PhotocraftApp>| h.ctx.data(|d| d.get_temp::<Picked>(picked_id()));
    let orange = [0.9, 0.5, 0.1];
    assert_eq!(ends(&h), [orange, [1.0; 3]], "the foreground and the background");
    assert!(picked(&h).is_none());

    let p = on_track(&h, "OKLab Gradient", 0.5);
    press(&mut h, p);
    let fg = rgb(h.state().session.tools.foreground);
    assert!(worst(fg, mix(orange, [1.0; 3], 0.5)) < 2e-3 && worst(fg, orange) > 0.05, "{fg:?}");
    assert_eq!(held(&h, false).lch, rgb_to_lch(fg), "the fields follow the pick");
    assert!(picked(&h).is_some_and(|(x, colour)| (x - 0.5).abs() < 1e-3 && colour == fg), "{:?}", picked(&h));
    assert_eq!(ends(&h), [orange, [1.0; 3]], "a pick leaves the ends alone");

    // The colour just picked goes to the end; then a blue to the start.
    h.get_by_label("Gradient End").click();
    h.run_steps(2);
    assert_eq!(ends(&h), [orange, fg]);
    assert!(picked(&h).is_none(), "another gradient: the thumb goes");
    let blue = [0.1, 0.2, 0.9];
    h.state_mut().session.tools.foreground = [0.1, 0.2, 0.9, 1.0];
    h.run_steps(2);
    h.get_by_label("Gradient Start").click();
    h.run_steps(2);
    assert_eq!(ends(&h), [blue, fg]);
    for (x, want) in [(0.0, blue), (1.0, fg), (0.25, mix(blue, fg, 0.25))] {
        let p = on_track(&h, "OKLab Gradient", x);
        press(&mut h, p);
        let got = rgb(h.state().session.tools.foreground);
        assert!(worst(got, want) < if x == 0.25 { 2e-3 } else { 1e-6 }, "at {x}: {got:?}, wanted {want:?}");
    }
    assert_eq!(rgb(h.state().session.tools.background), [1.0; 3], "the background is left alone");
}

/// Offscreen renders to look at: the dock with the picker in the Color group at its default
/// height, and the picker alone in both layouts, with and without the lines.
#[test]
#[ignore = "offscreen visual inspection; writes PNGs to PHOTOCRAFT_SYNC_PICKER_IMAGES or the temp directory"]
fn render_for_visual_inspection() {
    let out =
        std::env::var_os("PHOTOCRAFT_SYNC_PICKER_IMAGES").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("photocraft-sync-picker"));
    std::fs::create_dir_all(&out).unwrap();
    let colours = [("orange", [0.9, 0.5, 0.1]), ("blue", [0.15, 0.25, 0.9]), ("grey", [0.5, 0.5, 0.5])];
    for kind in [ThemeKind::ProMedium, ThemeKind::ProLight] {
        for (name, colour) in colours {
            for (shape, size, fill) in [("wide", vec2(330.0, 330.0), 310.0_f32), ("tall", vec2(280.0, 700.0), 680.0), ("large", vec2(620.0, 470.0), 450.0)] {
                for lines in [true, false] {
                    let mut h = Harness::builder().with_size(size).with_pixels_per_point(2.0).with_step_dt(1.0 / 60.0).build_ui_state(
                        move |ui, app: &mut PhotocraftApp| {
                            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                                ui.data_mut(|d| d.insert_temp(crate::panels::color_field_fill_id(), fill));
                                egui::Frame::new().inner_margin(8.0).show(ui, |ui| super::panel(app, ui));
                            }
                        },
                        app_with(colour),
                    );
                    PhotocraftApp::setup_context(&h.ctx, kind);
                    Opt::Lines.set(&h.ctx, lines);
                    // A gradient with some way to go: a red to a blue, with and without its thumb.
                    h.ctx.data_mut(|d| d.insert_persisted::<Ends>(ends_id(), [[0.85, 0.2, 0.15], [0.1, 0.3, 0.9]]));
                    if lines {
                        h.ctx.data_mut(|d| d.insert_temp::<Picked>(picked_id(), (0.35, colour)));
                    }
                    h.run_steps(4);
                    let lines = if lines { "lines" } else { "plain" };
                    h.render().unwrap().save(out.join(format!("{kind:?}-{shape}-{name}-{lines}.png"))).unwrap();
                }
            }
        }
        let mut app = app_with([0.9, 0.5, 0.1]);
        app.ui.theme = kind;
        app.ui.dock_tabs.color = Group::Color.tabs(Tokens::for_kind(kind).pro).iter().position(|name| *name == "Color").unwrap();
        let mut h = Harness::builder().with_size(vec2(760.0, 1000.0)).with_pixels_per_point(2.0).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    crate::panels::dock_area(app, ui);
                    egui::CentralPanel::default().show(ui, |_| {});
                }
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, kind);
        Opt::Enabled.set(&h.ctx, true);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("{kind:?}-dock.png"))).unwrap();
    }
    println!("wrote the renders to {}", out.display());
}
