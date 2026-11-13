//! A synced HSB + OKLCH picker for the Color panel (a personal addition, not one of Photoshop's
//! modes): the Hue Cube's saturation/brightness field and an OKLCH lightness/chroma field, each
//! with its hue strip. Both show the colour the panel edits, so a pick in one moves the other's
//! markers.
//!
//! The optional lines are the colours with the OKLCH chroma of the current one (solid, a dot
//! every 0.1 of lightness) and with its OKLCH lightness (dashed, a square every 0.02 of chroma),
//! drawn where each field puts them; ticks on the HSB hue strip give the HSB hue of each dot and
//! square. They show how far "the same lightness" or "the same chroma" strays in HSB.
//!
//! Kept apart from the panel it sits in: `color_panel_ui.rs` only declares this module, offers it
//! the Hue Cube mode ([`enabled`], [`panel`]) and adds its rows to the panel menu
//! ([`menu_items`]). Its two options live in egui's persisted memory, so they survive a restart
//! without a field in `ColorPanelState`.

mod model;
#[cfg(test)]
mod tests;

use egui::{Align2, Color32, FontId, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::PhotocraftApp;
use crate::state::ColorPanelMode;
use crate::theme::Tokens;
use model::{C_MAX, Held, Lch};
use photocraft_color::convert::{hsb_to_rgb, rgb_to_hsb};

/// Width of a hue strip.
const STRIP_W: f32 = 14.0;
/// From a field to its hue strip.
const STRIP_GAP: f32 = 4.0;
/// Room right of a hue strip for its pointer.
const POINTER_W: f32 = 7.0;
/// Between the two pickers, and above the foot.
const GAP: f32 = 6.0;
/// Height of the foot: the chips beside the two readout lines.
const FOOT_H: f32 = 38.0;
/// Points along each of the OKLCH lines.
const LINE_STEPS: usize = 96;

/// An option of the picker, kept in egui's persisted memory.
#[derive(Clone, Copy)]
enum Opt {
    /// The picker takes the Hue Cube's place.
    Enabled,
    /// The OKLCH lines are drawn.
    Lines,
}

impl Opt {
    fn id(self) -> egui::Id {
        egui::Id::unique(match self {
            Opt::Enabled => "sync-picker-enabled",
            Opt::Lines => "sync-picker-lines",
        })
    }

    fn get(self, ctx: &egui::Context) -> bool {
        // Off until asked for, so the panel is Photoshop's; the lines are on with it.
        ctx.data_mut(|d| d.get_persisted::<bool>(self.id())).unwrap_or(matches!(self, Opt::Lines))
    }

    fn set(self, ctx: &egui::Context, on: bool) {
        ctx.data_mut(|d| d.insert_persisted(self.id(), on));
    }
}

/// Is the picker shown in place of the Hue Cube?
pub(super) fn enabled(ctx: &egui::Context) -> bool {
    Opt::Enabled.get(ctx)
}

/// `ja` in a Japanese UI, `en` otherwise: this module's strings aren't in the catalogs.
fn text(en: &'static str, ja: &'static str) -> &'static str {
    if crate::i18n::current().code() == "ja" { ja } else { en }
}

/// The picker's rows in the Color panel's menu: the picker itself (checked while it shows), and
/// its lines while it is on.
pub(super) fn menu_items(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let shown = enabled(&ctx) && app.ui.color_panel.mode == ColorPanelMode::HueCube;
    if super::check_item(ui, shown, "HSB + OKLCH") {
        Opt::Enabled.set(&ctx, !shown);
        if !shown {
            // It takes the Hue Cube's place, so that is the mode to be in.
            app.ui.color_panel.mode = ColorPanelMode::HueCube;
        }
        ui.close();
    }
    let lines = Opt::Lines.get(&ctx);
    if shown && super::check_item(ui, lines, text("OKLCH Lines", "OKLCH の線を表示")) {
        Opt::Lines.set(&ctx, !lines);
        ui.close();
    }
}

/// Where the parts go.
#[derive(Clone, Copy, Debug)]
struct Slots {
    hsb: Rect,
    hsb_hue: Rect,
    lch: Rect,
    lch_hue: Rect,
    foot: Rect,
}

/// Lay the two pickers out in `area` over the foot: side by side or one over the other, whichever
/// gives the fields the longer short side (a wide, low dock group or a narrow, tall one).
fn slots(area: Rect) -> Slots {
    let foot = Rect::from_min_max(pos2(area.left(), (area.bottom() - FOOT_H).max(area.top())), area.max);
    let body = Rect::from_min_max(area.min, pos2(area.right(), (foot.top() - GAP).max(area.top())));
    let beside = STRIP_GAP + STRIP_W + POINTER_W;
    let across = vec2((body.width() - GAP) / 2.0 - beside, body.height());
    let down = vec2(body.width() - beside, (body.height() - GAP) / 2.0);
    let side_by_side = across.min_elem() >= down.min_elem();
    let field = if side_by_side { across } else { down }.max(vec2(1.0, 1.0));
    let step = if side_by_side { vec2(field.x + beside + GAP, 0.0) } else { vec2(0.0, field.y + GAP) };
    let picker = |min: Pos2| {
        let field = Rect::from_min_size(min, field);
        (field, Rect::from_min_size(pos2(field.right() + STRIP_GAP, field.top()), vec2(STRIP_W, field.height())))
    };
    let ((hsb, hsb_hue), (lch, lch_hue)) = (picker(body.min), picker(body.min + step));
    Slots { hsb, hsb_hue, lch, lch_hue, foot }
}

/// The space the picker takes: the panel's width, and the height the dock group offers (as the
/// Hue Cube takes it), or 200 pt where nothing offers one.
fn area(ui: &mut egui::Ui) -> Rect {
    let fill = ui.data(|d| d.get_temp::<f32>(crate::panels::color_field_fill_id()));
    let gaps = 2.0 * ui.spacing().item_spacing.y + 2.0;
    let height = fill.filter(|f| f.is_finite()).map_or(200.0, |f| f - gaps).clamp(FOOT_H + GAP + 40.0, 2000.0);
    let width = ui.available_width();
    let width = if width.is_finite() { width.clamp(1.0, 4000.0) } else { 1.0 };
    ui.allocate_exact_size(vec2(width, height), Sense::hover()).0
}

/// A press or drag on `rect` (named `name` for accessibility), as fractions across and down it.
fn pick(ui: &egui::Ui, rect: Rect, name: &'static str) -> Option<(f32, f32)> {
    let resp = ui.interact(rect, ui.scope_id().with(("sync-picker", name)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, ui.is_enabled(), name));
    let p = crate::widgets::pick_pos(&resp)?;
    let (x, y) = ((p.x - rect.left()) / rect.width(), (p.y - rect.top()) / rect.height());
    (x.is_finite() && y.is_finite()).then_some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

/// What the pointer picks this frame in the fields and strips at `s`, starting from `held`.
fn picks(ui: &egui::Ui, s: &Slots, mut held: Held) -> Held {
    if let Some((x, y)) = pick(ui, s.hsb, "HSB Field") {
        let [h, ..] = held.hsb;
        held = held.with_hsb([h, x, 1.0 - y]);
    }
    if let Some((_, y)) = pick(ui, s.hsb_hue, "HSB Hue") {
        let [_, saturation, brightness] = held.hsb;
        held = held.with_hsb([1.0 - y, saturation, brightness]);
    }
    if let Some((x, y)) = pick(ui, s.lch, "OKLCH Field") {
        // Past the gamut's edge picks the edge, so the marker stays under the colour it gives.
        let (l, h) = (1.0 - y, held.lch.h);
        held = held.with_lch(Lch { l, c: (x * C_MAX).min(model::max_chroma(l, h)), h });
    }
    if let Some((_, y)) = pick(ui, s.lch_hue, "OKLCH Hue") {
        held = held.with_lch(Lch { h: 1.0 - y, ..held.lch });
    }
    held
}

/// The synced picker, in place of the Hue Cube: the HSB and OKLCH fields with their hue strips,
/// over the chips and the two readouts.
pub(super) fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let background = app.ui.color_panel.background;
    let rgb = super::active(app);
    let held_id = egui::Id::unique(("sync-picker-held", background));
    let shown = ui.data(|d| d.get_temp::<Held>(held_id)).map_or_else(|| Held::of(rgb), |held| held.follow(rgb));
    let s = slots(area(ui));
    let held = picks(ui, &s, shown);
    // A held, unmoved press repeats the pick: recolour what is selected only on a change.
    if held.rgb != shown.rgb {
        super::set_color(app, ui.ctx(), background, held.rgb);
    }
    ui.data_mut(|d| d.insert_temp(held_id, held));

    let [hue, saturation, brightness] = held.hsb;
    let painter = ui.painter();
    painter.add(grid(s.hsb, 16, 16, |x, y| super::rgb32(hsb_to_rgb([hue * 360.0, x, 1.0 - y]))));
    painter.rect_stroke(s.hsb, 0.0, Stroke::new(1.0, t.separator), StrokeKind::Outside);
    painter.add(grid(s.hsb_hue, 1, 24, |_, y| super::rgb32(hsb_to_rgb([(1.0 - y) * 360.0, 1.0, 1.0]))));
    lch_field(ui, s.lch, held.lch.h, &t);
    let strip = model::hue_strip();
    let last = strip.len().saturating_sub(1);
    painter.add(grid(s.lch_hue, 1, last.max(1), |_, y| {
        // The grid's rows are the strip's colours.
        strip.get((y * last as f32).round() as usize).map_or(Color32::BLACK, |c| super::rgb32(*c))
    }));
    if Opt::Lines.get(ui.ctx()) {
        lines(painter, &s, held);
    }
    knob(painter, s.hsb, (saturation, 1.0 - brightness));
    knob(painter, s.lch, (held.chroma() / C_MAX, 1.0 - held.lch.l));
    pointer(painter, s.hsb_hue, hue, t.text);
    pointer(painter, s.lch_hue, held.lch.h, t.text);
    foot(app, ui, s.foot, held, &t);
}

/// A smooth-shaded `nx` x `ny` grid over `rect`, coloured by `color(x, y)` with x and y in 0..1
/// (y down).
fn grid(rect: Rect, nx: usize, ny: usize, color: impl Fn(f32, f32) -> Color32) -> Mesh {
    let (nx, ny) = (nx.clamp(1, 256), ny.clamp(1, 256));
    let mut mesh = Mesh::default();
    for j in 0..=ny {
        for i in 0..=nx {
            let (x, y) = (i as f32 / nx as f32, j as f32 / ny as f32);
            mesh.colored_vertex(pos2(rect.left() + x * rect.width(), rect.top() + y * rect.height()), color(x, y));
        }
    }
    let row = (nx + 1) as u32;
    for j in 0..ny as u32 {
        for i in 0..nx as u32 {
            let a = j * row + i;
            mesh.add_triangle(a, a + 1, a + row + 1);
            mesh.add_triangle(a, a + row + 1, a + row);
        }
    }
    mesh
}

/// The OKLCH field's image and the hue and size it was made for.
#[derive(Clone)]
struct FieldImage {
    hue: f32,
    size: [usize; 2],
    texture: egui::TextureHandle,
}

/// The OKLCH field's texture for `hue` at `rect`'s size in pixels, remade only when either
/// changes: the gamut's edge is curved, so the field is an image, not a shaded mesh.
fn field_texture(ui: &egui::Ui, rect: Rect, hue: f32) -> egui::TextureHandle {
    let ppp = ui.ctx().pixels_per_point();
    let px = |points: f32| ((points * ppp).round() as usize).clamp(2, model::FIELD_MAX_PX);
    let size = [px(rect.width()), px(rect.height())];
    let id = egui::Id::unique("sync-picker-oklch-field");
    let cached = ui.data(|d| d.get_temp::<FieldImage>(id));
    if let Some(image) = cached.as_ref().filter(|image| image.hue == hue && image.size == size) {
        return image.texture.clone();
    }
    let (size, pixels) = model::field_pixels(hue, size[0], size[1]);
    let image = egui::ColorImage::new(size, pixels.into_iter().map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a)).collect());
    let options = egui::TextureOptions::LINEAR;
    let texture = match cached {
        Some(mut stale) => {
            stale.texture.set(image, options);
            stale.texture
        }
        None => ui.ctx().load_texture("sync-picker-oklch-field", image, options),
    };
    ui.data_mut(|d| d.insert_temp(id, FieldImage { hue, size, texture: texture.clone() }));
    texture
}

/// The OKLCH field at `hue`: lightness up, chroma across to [`C_MAX`]. Where sRGB has no such
/// colour it shows the dock's colour: a well a step off the panel's in every theme, and never the
/// white or black the gamut's two ends would vanish into (a light theme's field colour is white).
fn lch_field(ui: &egui::Ui, rect: Rect, hue: f32, t: &Tokens) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, t.dock);
    let texture = field_texture(ui, rect, hue);
    painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    // The corner no hue's gamut reaches: near black every hue holds little chroma.
    painter.text(rect.right_bottom() - vec2(4.0, 3.0), Align2::RIGHT_BOTTOM, "OKLCH", FontId::proportional(10.0), t.text_faint);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, t.separator), StrokeKind::Outside);
}

/// The point of `rect` at fractions (`x`, `y`) across and down it.
fn at(rect: Rect, (x, y): (f32, f32)) -> Pos2 {
    pos2(rect.left() + x.clamp(0.0, 1.0) * rect.width(), rect.top() + y.clamp(0.0, 1.0) * rect.height())
}

/// A field's marker, clipped to the field at its edges as the Hue Cube's is.
fn knob(painter: &egui::Painter, rect: Rect, place: (f32, f32)) {
    let painter = painter.with_clip_rect(rect);
    let p = at(rect, place);
    painter.circle_stroke(p, 5.0, Stroke::new(1.5, Color32::WHITE));
    painter.circle_stroke(p, 6.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
}

/// A hue strip's pointer at `hue` (turns; 1 at the top).
fn pointer(painter: &egui::Painter, strip: Rect, hue: f32, color: Color32) {
    let p = at(strip, (1.0, 1.0 - hue));
    let tip = vec![pos2(p.x + 1.0, p.y), pos2(p.x + 6.0, p.y - 4.0), pos2(p.x + 6.0, p.y + 4.0)];
    painter.add(egui::Shape::convex_polygon(tip, color, Stroke::NONE));
}

/// Where an OKLCH colour falls in a field, as fractions across and down it.
type Place = fn(Lch) -> (f32, f32);

/// Where the OKLCH colour `q` falls in the OKLCH field.
fn in_lch(q: Lch) -> (f32, f32) {
    (q.c / C_MAX, 1.0 - q.l)
}

/// Where the OKLCH colour `q` falls in the HSB field: by its saturation and brightness alone,
/// since its HSB hue may not be the one the field shows.
fn in_hsb(q: Lch) -> (f32, f32) {
    let [_, saturation, brightness] = rgb_to_hsb(model::lch_to_rgb(q));
    (saturation, 1.0 - brightness)
}

/// The OKLCH lines through the colour: every lightness at its chroma (solid, dots) and every
/// chroma at its lightness (dashed, squares), in both fields, with the HSB hues of the dots and
/// squares ticked on the HSB hue strip.
fn lines(painter: &egui::Painter, s: &Slots, held: Held) {
    let Lch { l, h, .. } = held.lch;
    let c = held.chroma();
    let edge = model::max_chroma(l, h);
    // A little slack, so the colour's own chroma counts as inside at its own lightness.
    let holds = |q: &Lch| (0.0..=1.0).contains(&q.l) && q.c >= 0.0 && q.c <= model::max_chroma(q.l, h) + 1e-3;
    let step = |k: usize| k as f32 / LINE_STEPS as f32;
    let lightness: Vec<Option<Lch>> = (0..=LINE_STEPS).map(|k| Some(Lch { l: step(k), c, h }).filter(holds)).collect();
    let chroma: Vec<Option<Lch>> = (0..=LINE_STEPS).map(|k| Some(Lch { l, c: edge * step(k), h })).collect();
    let dots: Vec<Lch> = (-10..=10).filter(|k| *k != 0).map(|k| Lch { l: l + 0.1 * k as f32, c, h }).filter(holds).collect();
    let squares: Vec<Lch> = (-20..=20).filter(|k| *k != 0).map(|k| Lch { l, c: c + 0.02 * k as f32, h }).filter(holds).collect();
    let fields: [(Rect, Place); 2] = [(s.hsb, in_hsb), (s.lch, in_lch)];
    for (rect, place) in fields {
        let painter = painter.with_clip_rect(rect);
        let size = (rect.width().min(rect.height()) / 40.0).clamp(2.0, 3.5);
        stroke_runs(&painter, &runs(lightness.iter().map(|q| q.map(|q| at(rect, place(q))))), false);
        stroke_runs(&painter, &runs(chroma.iter().map(|q| q.map(|q| at(rect, place(q))))), true);
        for q in &dots {
            painter.circle(at(rect, place(*q)), size, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
        }
        for q in &squares {
            let square = Rect::from_center_size(at(rect, place(*q)), vec2(size * 1.7, size * 1.7));
            painter.rect_filled(square, 0.0, Color32::WHITE);
            painter.rect_stroke(square, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
        }
    }
    for q in dots.iter().chain(&squares) {
        let [hue, saturation, brightness] = rgb_to_hsb(model::lch_to_rgb(*q));
        // A grey has no hue to tick.
        if saturation > 0.01 && brightness > 0.01 {
            let p = at(s.hsb_hue, (0.5, 1.0 - hue / 360.0));
            let tick = Rect::from_min_max(pos2(p.x, p.y - 1.5), pos2(s.hsb_hue.right(), p.y + 1.5));
            painter.rect_filled(tick, 0.0, Color32::BLACK);
            painter.rect_filled(tick.shrink2(vec2(1.0, 1.0)), 0.0, Color32::WHITE);
        }
    }
}

/// The stretches of `points` without a gap.
fn runs(points: impl Iterator<Item = Option<Pos2>>) -> Vec<Vec<Pos2>> {
    let mut out = vec![Vec::new()];
    for p in points {
        match (p, out.last_mut()) {
            (Some(p), Some(run)) => run.push(p),
            _ => out.push(Vec::new()),
        }
    }
    out.retain(|run| run.len() > 1);
    out
}

/// Stroke `runs` white over a dark casing, so they read on any colour of a field.
fn stroke_runs(painter: &egui::Painter, runs: &[Vec<Pos2>], dashed: bool) {
    for stroke in [Stroke::new(3.0, Color32::from_black_alpha(190)), Stroke::new(1.25, Color32::WHITE)] {
        for run in runs {
            if dashed {
                painter.extend(egui::Shape::dashed_line(run, stroke, 5.0, 3.5));
            } else {
                painter.add(egui::Shape::line(run.clone(), stroke));
            }
        }
    }
}

/// Under the fields: the foreground and background chips (a click picks which one the fields
/// edit), and the colour's HSB and OKLCH values.
fn foot(app: &mut PhotocraftApp, ui: &mut egui::Ui, rect: Rect, held: Held, t: &Tokens) {
    crate::panels::field_chips(app, ui, Rect::from_min_size(rect.min, vec2(38.0, 38.0)));
    let [hue, saturation, brightness] = held.hsb;
    let degrees = |turns: f32| (turns * 360.0).round().rem_euclid(360.0);
    let hsb = format!("H {:>3}°  S {:>3}%  B {:>3}%", degrees(hue), (saturation * 100.0).round(), (brightness * 100.0).round());
    let lch = format!("L {:.2}  C {:.3}  H {:>3}°", held.lch.l, held.chroma(), degrees(held.lch.h));
    let x = rect.left() + 38.0 + 8.0;
    let font = crate::theme::mono(11.0);
    ui.painter().text(pos2(x, rect.top() + 11.0), Align2::LEFT_CENTER, hsb, font.clone(), t.text_dim);
    ui.painter().text(pos2(x, rect.top() + 27.0), Align2::LEFT_CENTER, lch, font, t.text_dim);
}
