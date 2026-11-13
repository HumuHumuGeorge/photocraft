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
//! Under the fields, each optional too: the OKLCH sliders (L, C and H, each track shaded with
//! what its values would give and a field to type the value into), and the OKLab gradient, the
//! colours along the straight line through OKLab between two colours. A click on the chip at
//! either end of it puts the current colour there, and a press or drag along it picks from it.
//!
//! Kept apart from the panel it sits in: `color_panel_ui.rs` only declares this module, offers it
//! the Hue Cube mode ([`enabled`], [`panel`]) and adds its rows to the panel menu
//! ([`menu_items`]). Its options and the gradient's ends live in egui's persisted memory, so they
//! survive a restart without a field in `ColorPanelState`.

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
/// Between the two pickers, and between the fields, the sliders, the gradient and the foot.
const GAP: f32 = 6.0;
/// Height of the foot: the chips beside the two readout lines.
const FOOT_H: f32 = 38.0;
/// Height of a slider's row and of the gradient's.
const ROW_H: f32 = 20.0;
/// Between the sliders' rows.
const ROW_GAP: f32 = 2.0;
/// Width of a slider's caption.
const CAPTION_W: f32 = 12.0;
/// Width of a slider's value field.
const VALUE_W: f32 = 46.0;
/// The height the fields keep however short the panel: a panel too short for it scrolls.
const FIELD_MIN_H: f32 = 40.0;
/// Points along each of the OKLCH lines.
const LINE_STEPS: usize = 96;
/// Colours along the gradient.
const GRADIENT_STEPS: usize = 32;

/// An option of the picker, kept in egui's persisted memory.
#[derive(Clone, Copy)]
enum Opt {
    /// The picker takes the Hue Cube's place.
    Enabled,
    /// The OKLCH lines are drawn.
    Lines,
    /// The OKLCH sliders are shown.
    Sliders,
    /// The OKLab gradient is shown.
    Gradient,
}

impl Opt {
    fn id(self) -> egui::Id {
        egui::Id::unique(match self {
            Opt::Enabled => "sync-picker-enabled",
            Opt::Lines => "sync-picker-lines",
            Opt::Sliders => "sync-picker-sliders",
            Opt::Gradient => "sync-picker-gradient",
        })
    }

    fn get(self, ctx: &egui::Context) -> bool {
        // Off until asked for, so the panel is Photoshop's; its parts are all on with it.
        ctx.data_mut(|d| d.get_persisted::<bool>(self.id())).unwrap_or(!matches!(self, Opt::Enabled))
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
/// its parts while it is on.
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
    let parts = [
        (Opt::Lines, text("OKLCH Lines", "OKLCH の線を表示")),
        (Opt::Sliders, text("OKLCH Sliders", "OKLCH スライダー")),
        (Opt::Gradient, text("OKLab Gradient", "OKLab グラデーション")),
    ];
    for (part, label) in parts {
        let on = part.get(&ctx);
        if shown && super::check_item(ui, on, label) {
            part.set(&ctx, !on);
            ui.close();
        }
    }
}

/// Which of the rows under the fields are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rows {
    sliders: bool,
    gradient: bool,
}

impl Rows {
    fn shown(ctx: &egui::Context) -> Rows {
        Rows { sliders: Opt::Sliders.get(ctx), gradient: Opt::Gradient.get(ctx) }
    }

    /// Height of everything under the fields: the sliders, the gradient and the foot.
    fn height(self) -> f32 {
        let sliders = if self.sliders { 3.0 * ROW_H + 2.0 * ROW_GAP + GAP } else { 0.0 };
        let gradient = if self.gradient { ROW_H + GAP } else { 0.0 };
        sliders + gradient + FOOT_H
    }
}

/// Where the parts go.
#[derive(Clone, Copy, Debug)]
struct Slots {
    hsb: Rect,
    hsb_hue: Rect,
    lch: Rect,
    lch_hue: Rect,
    /// The rows of the L, C and H sliders, when they are shown.
    sliders: Option<[Rect; 3]>,
    /// The gradient's row, when it is shown.
    gradient: Option<Rect>,
    foot: Rect,
}

/// Lay the parts out in `area`. Up from its bottom go the foot, the gradient and the sliders, and
/// the two pickers share what is left: side by side or one over the other, whichever gives the
/// fields the longer short side (a wide, low dock group or a narrow, tall one).
fn slots(area: Rect, rows: Rows) -> Slots {
    let mut bottom = area.bottom();
    let mut band = |height: f32| {
        let rect = Rect::from_min_max(pos2(area.left(), (bottom - height).max(area.top())), pos2(area.right(), bottom));
        bottom = (rect.top() - GAP).max(area.top());
        rect
    };
    let foot = band(FOOT_H);
    let gradient = rows.gradient.then(|| band(ROW_H));
    let sliders = rows.sliders.then(|| {
        let block = band(3.0 * ROW_H + 2.0 * ROW_GAP);
        [0.0, 1.0, 2.0].map(|i| Rect::from_min_size(pos2(block.left(), block.top() + i * (ROW_H + ROW_GAP)), vec2(block.width(), ROW_H)))
    });
    let body = Rect::from_min_max(area.min, pos2(area.right(), bottom));
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
    Slots { hsb, hsb_hue, lch, lch_hue, sliders, gradient, foot }
}

/// A slider row's caption, track and value field.
fn row_parts(row: Rect) -> (Rect, Rect, Rect) {
    let caption = Rect::from_min_size(row.min, vec2(CAPTION_W, row.height()));
    let field = Rect::from_min_max(pos2((row.right() - VALUE_W).max(caption.right()), row.top()), row.max);
    let left = caption.right() + 2.0;
    (caption, Rect::from_min_max(pos2(left, row.top()), pos2((field.left() - 2.0).max(left), row.bottom())), field)
}

/// The gradient row's parts: the chips of its two ends, and the track between them.
fn gradient_parts(row: Rect) -> ([Rect; 2], Rect) {
    let side = row.height();
    let chips = [Rect::from_min_size(row.min, vec2(side, side)), Rect::from_min_size(pos2((row.right() - side).max(row.left()), row.top()), vec2(side, side))];
    let left = row.left() + side + 2.0;
    (chips, Rect::from_min_max(pos2(left, row.top()), pos2((row.right() - side - 2.0).max(left), row.bottom())))
}

/// The shaded bar of a track: in from its ends by the thumb's half width, with the thumb under it.
fn bar_of(track: Rect) -> Rect {
    let left = track.left() + 6.0;
    Rect::from_min_max(pos2(left, track.center().y - 8.0), pos2((track.right() - 6.0).max(left + 1.0), track.center().y))
}

/// The space the picker takes: the panel's width, and the height the dock group offers (as the
/// Hue Cube takes it), or one for fields 156 pt tall where nothing offers one. Never less than
/// the rows shown and fields of [`FIELD_MIN_H`] need: the panel scrolls in a shorter group.
fn area(ui: &mut egui::Ui, rows: Rows) -> Rect {
    let fill = ui.data(|d| d.get_temp::<f32>(crate::panels::color_field_fill_id()));
    let gaps = 2.0 * ui.spacing().item_spacing.y + 2.0;
    let under = rows.height() + GAP;
    let height = fill.filter(|f| f.is_finite()).map_or(under + 156.0, |f| f - gaps).clamp(under + FIELD_MIN_H, 2000.0);
    let width = ui.available_width();
    let width = if width.is_finite() { width.clamp(1.0, 4000.0) } else { 1.0 };
    ui.allocate_exact_size(vec2(width, height), Sense::hover()).0
}

/// A press or drag on `hit` (named `name` for accessibility), as fractions across and down `of`.
fn pick_in(ui: &egui::Ui, hit: Rect, of: Rect, name: &'static str) -> Option<(f32, f32)> {
    let resp = ui.interact(hit, ui.scope_id().with(("sync-picker", name)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, ui.is_enabled(), name));
    let p = crate::widgets::pick_pos(&resp)?;
    let (x, y) = ((p.x - of.left()) / of.width(), (p.y - of.top()) / of.height());
    (x.is_finite() && y.is_finite()).then_some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

/// A press or drag on `rect` (named `name` for accessibility), as fractions across and down it.
fn pick(ui: &egui::Ui, rect: Rect, name: &'static str) -> Option<(f32, f32)> {
    pick_in(ui, rect, rect, name)
}

/// One of the OKLCH sliders.
struct Channel {
    caption: &'static str,
    /// The names of its track and of its value field, for accessibility.
    track: &'static str,
    field: &'static str,
    /// The value at the right end of its track; the left end is 0.
    top: f32,
    /// Decimal places its field shows.
    decimals: usize,
    /// Colours along its track.
    steps: usize,
    /// Its value in an OKLCH colour.
    get: fn(Lch) -> f32,
    /// The colour with its value set. A chroma past the gamut's edge is the edge's, as a press
    /// past it in the field is.
    set: fn(Lch, f32) -> Lch,
    /// How much of its track, from the left, the colour can take: all of it, or for the chroma up
    /// to the gamut's edge.
    reach: fn(Lch) -> f32,
}

/// The sliders, top to bottom.
const CHANNELS: [Channel; 3] = [
    Channel {
        caption: "L",
        track: "OKLCH L",
        field: "OKLCH L value",
        top: 1.0,
        decimals: 3,
        steps: 24,
        get: |q| q.l,
        set: |q, l| Lch { l, ..q },
        reach: |_| 1.0,
    },
    Channel {
        caption: "C",
        track: "OKLCH C",
        field: "OKLCH C value",
        top: C_MAX,
        decimals: 3,
        steps: 16,
        get: |q| q.c,
        set: |q, c| Lch { c: c.min(model::max_chroma(q.l, q.h)), ..q },
        reach: |q| model::max_chroma(q.l, q.h) / C_MAX,
    },
    Channel {
        caption: "H",
        track: "OKLCH H",
        field: "OKLCH H value",
        top: 360.0,
        decimals: 1,
        steps: 48,
        get: |q| q.h * 360.0,
        set: |q, degrees| Lch { h: degrees / 360.0, ..q },
        reach: |_| 1.0,
    },
];

/// What the pointer picks this frame in the fields, the strips and the sliders' tracks at `s`,
/// starting from `held`.
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
    for (row, channel) in s.sliders.iter().flatten().zip(&CHANNELS) {
        let (_, track, _) = row_parts(*row);
        if let Some((x, _)) = pick_in(ui, track, bar_of(track), channel.track) {
            held = held.with_lch((channel.set)(held.lch, x * channel.top));
        }
    }
    held
}

/// A slider's value field at `rect`: `value` to the channel's decimal places. While the field has
/// the keyboard it shows what is typed, and when the keyboard leaves it (Enter, Tab, a click
/// elsewhere) a number typed that lies on the channel's track becomes `value` (returning true);
/// anything else, or Esc, puts the colour's value back.
///
/// Not as each key is typed, as the panel's other fields do: every lightness and chroma starts
/// with "0", which would turn the colour black or grey on the way to "0.42".
fn number_field(ui: &mut egui::Ui, rect: Rect, channel: &Channel, value: &mut f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let id = ui.scope_id().with(("sync-picker", channel.field));
    crate::widgets::surface(ui, rect, t.field, false);
    if !t.bevel {
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    }
    let shown = format!("{:.*}", channel.decimals, *value);
    // What was in the field when it last had the keyboard.
    let before = ui.data(|d| d.get_temp::<String>(id));
    let mut typed = before.clone().unwrap_or_else(|| shown.clone());
    let inner = rect.shrink2(vec2(4.0, 1.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    // `Frame::NONE`: the themed surface above is this field's frame.
    let edit = egui::TextEdit::singleline(&mut typed).id(id).char_limit(8).desired_width(inner.width()).frame(egui::Frame::NONE).font(crate::theme::mono(11.0));
    if child.add(edit).accessible_name(channel.field).has_focus() {
        ui.data_mut(|d| d.insert_temp(id, typed));
        return false;
    }
    ui.data_mut(|d| d.remove::<String>(id));
    let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // Left as it was shown, the value keeps the digits the field doesn't show.
    let number = before.filter(|text| *text != shown && !cancelled).and_then(|text| text.trim().parse::<f32>().ok());
    match number {
        Some(number) if number.is_sign_positive() && number <= channel.top => {
            *value = number;
            true
        }
        _ => false,
    }
}

/// The sliders' value fields: each shows its channel of the colour, and a number typed into one
/// sets that channel.
fn value_fields(ui: &mut egui::Ui, rows: &[Rect; 3], mut held: Held) -> Held {
    for (row, channel) in rows.iter().zip(&CHANNELS) {
        let (_, _, field) = row_parts(*row);
        // What the colour has: a chroma asked for past the gamut's edge shows as the edge's.
        let mut value = (channel.get)(Lch { c: held.chroma(), ..held.lch });
        if number_field(ui, field, channel, &mut value) {
            held = held.with_lch((channel.set)(held.lch, value));
        }
    }
    held
}

/// The gradient's two ends, sRGB-encoded.
type Ends = [[f32; 3]; 2];

/// Where on the gradient the colour was picked: how far along it, and the colour that gave.
type Picked = (f32, [f32; 3]);

fn ends_id() -> egui::Id {
    egui::Id::unique("sync-picker-gradient-ends")
}

fn picked_id() -> egui::Id {
    egui::Id::unique("sync-picker-gradient-picked")
}

/// The gradient's ends, kept across restarts in egui's persisted memory: the foreground and the
/// background colour as they are when the gradient first shows, until a chip is set.
fn gradient_ends(app: &PhotocraftApp, ctx: &egui::Context) -> Ends {
    let tidy = |c: [f32; 3]| c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    if let Some(ends) = ctx.data_mut(|d| d.get_persisted::<Ends>(ends_id())) {
        return ends.map(tidy);
    }
    let ([r0, g0, b0, _], [r1, g1, b1, _]) = (app.session.tools.foreground, app.session.tools.background);
    let ends = [tidy([r0, g0, b0]), tidy([r1, g1, b1])];
    ctx.data_mut(|d| d.insert_persisted(ends_id(), ends));
    ends
}

/// The gradient's row at `row`: a click on the chip of an end puts the current colour there, and
/// a press or drag along the gradient picks the colour under the pointer.
fn gradient_picks(ui: &egui::Ui, row: Rect, ends: &mut Ends, held: Held) -> Held {
    let (chips, track) = gradient_parts(row);
    let mut moved = false;
    for ((chip, end), name) in chips.into_iter().zip(ends.iter_mut()).zip(["Gradient Start", "Gradient End"]) {
        let resp = ui.interact(chip, ui.scope_id().with(("sync-picker", name)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name));
        if resp.on_hover_text(text("Click to put the current color here", "クリックで現在の色をここに入れる")).clicked() {
            *end = held.rgb;
            moved = true;
        }
    }
    if moved {
        // Another gradient: the colour is no longer where it was picked on it.
        ui.data_mut(|d| {
            d.insert_persisted(ends_id(), *ends);
            d.remove::<Picked>(picked_id());
        });
    }
    let Some((x, _)) = pick_in(ui, track, bar_of(track), "OKLab Gradient") else { return held };
    let [from, to] = *ends;
    let next = held.follow(model::mix(from, to, x));
    ui.data_mut(|d| d.insert_temp::<Picked>(picked_id(), (x, next.rgb)));
    next
}

/// The synced picker, in place of the Hue Cube: the HSB and OKLCH fields with their hue strips,
/// over the OKLCH sliders, the OKLab gradient, and the chips beside the two readouts.
pub(super) fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let background = app.ui.color_panel.background;
    let rgb = super::active(app);
    let held_id = egui::Id::unique(("sync-picker-held", background));
    let shown = ui.data(|d| d.get_temp::<Held>(held_id)).map_or_else(|| Held::of(rgb), |held| held.follow(rgb));
    let rows = Rows::shown(ui.ctx());
    let s = slots(area(ui, rows), rows);
    let mut held = picks(ui, &s, shown);
    let mut ends = None;
    if let Some(row) = s.gradient {
        let mut both = gradient_ends(app, ui.ctx());
        held = gradient_picks(ui, row, &mut both, held);
        ends = Some(both);
    }
    if let Some(rows) = &s.sliders {
        held = value_fields(ui, rows, held);
    }
    // A held, unmoved press repeats the pick: recolour what is selected only on a change.
    if held.rgb != shown.rgb {
        super::set_color(app, ui.ctx(), background, held.rgb);
    }
    ui.data_mut(|d| d.insert_temp(held_id, held));

    fields(ui, &s, held, &t);
    if let Some(rows) = &s.sliders {
        sliders(ui.painter(), rows, held, &t);
    }
    if let (Some(row), Some(ends)) = (s.gradient, ends) {
        // The thumb stays where the colour was picked while it is still the colour.
        let at = ui.data(|d| d.get_temp::<Picked>(picked_id())).filter(|(_, rgb)| *rgb == held.rgb).map(|(x, _)| x);
        gradient(ui.painter(), row, ends, at, &t);
    }
    foot(app, ui, s.foot, held, &t);
}

/// The two fields and their hue strips, with the lines, markers and pointers of `held`.
fn fields(ui: &egui::Ui, s: &Slots, held: Held, t: &Tokens) {
    let [hue, saturation, brightness] = held.hsb;
    let painter = ui.painter();
    painter.add(grid(s.hsb, 16, 16, |x, y| super::rgb32(hsb_to_rgb([hue * 360.0, x, 1.0 - y]))));
    painter.rect_stroke(s.hsb, 0.0, Stroke::new(1.0, t.separator), StrokeKind::Outside);
    painter.add(grid(s.hsb_hue, 1, 24, |_, y| super::rgb32(hsb_to_rgb([(1.0 - y) * 360.0, 1.0, 1.0]))));
    lch_field(ui, s.lch, held.lch.h, t);
    let strip = model::hue_strip();
    let last = strip.len().saturating_sub(1);
    painter.add(grid(s.lch_hue, 1, last.max(1), |_, y| {
        // The grid's rows are the strip's colours.
        strip.get((y * last as f32).round() as usize).map_or(Color32::BLACK, |c| super::rgb32(*c))
    }));
    if Opt::Lines.get(ui.ctx()) {
        lines(painter, s, held);
    }
    knob(painter, s.hsb, (saturation, 1.0 - brightness));
    knob(painter, s.lch, (held.chroma() / C_MAX, 1.0 - held.lch.l));
    pointer(painter, s.hsb_hue, hue, t.text);
    pointer(painter, s.lch_hue, held.lch.h, t.text);
}

/// A track's bar, shaded with `stops` over the first `reach` of its width (the rest is the
/// dock's colour: values the colour can't take), and Photoshop's triangle thumb under it at
/// `thumb` of its width.
fn track(painter: &egui::Painter, rect: Rect, stops: &[Color32], reach: f32, thumb: Option<f32>, t: &Tokens) {
    let bar = bar_of(rect);
    painter.rect_filled(bar, 0.0, t.dock);
    let last = stops.len().saturating_sub(1).max(1);
    let shaded = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * reach.clamp(0.0, 1.0), bar.bottom()));
    painter.add(grid(shaded, last, 1, |x, _| stops.get((x * last as f32).round() as usize).copied().unwrap_or(Color32::BLACK)));
    painter.rect_stroke(bar, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    if let Some(at) = thumb.filter(|at| at.is_finite()) {
        let (x, y) = (bar.left() + bar.width() * at.clamp(0.0, 1.0), bar.bottom() + 1.0);
        let shape = vec![pos2(x, y), pos2(x + 5.0, y + 7.0), pos2(x - 5.0, y + 7.0)];
        painter.add(egui::Shape::convex_polygon(shape, t.text, Stroke::new(1.0, t.field_border)));
    }
}

/// The OKLCH sliders: each track shows what its values would give with the other two channels
/// as they are, its thumb at the colour's own value.
fn sliders(painter: &egui::Painter, rows: &[Rect; 3], held: Held, t: &Tokens) {
    let has = Lch { c: held.chroma(), ..held.lch };
    for (row, channel) in rows.iter().zip(&CHANNELS) {
        let (caption, rect, _) = row_parts(*row);
        painter.text(caption.left_center(), Align2::LEFT_CENTER, channel.caption, FontId::proportional(12.0), t.text_dim);
        let reach = (channel.reach)(held.lch);
        let stops: Vec<Color32> = (0..=channel.steps)
            .map(|k| {
                let value = k as f32 / channel.steps as f32 * reach * channel.top;
                super::rgb32(model::lch_to_rgb((channel.set)(held.lch, value)))
            })
            .collect();
        track(painter, rect, &stops, reach, Some((channel.get)(has) / channel.top), t);
    }
}

/// The OKLab gradient between the chips of its two `ends`, its thumb at `at` when the colour was
/// picked from it.
fn gradient(painter: &egui::Painter, row: Rect, ends: Ends, at: Option<f32>, t: &Tokens) {
    let (chips, rect) = gradient_parts(row);
    for (chip, end) in chips.into_iter().zip(ends) {
        let chip = chip.shrink(1.0);
        painter.rect_filled(chip, 0.0, super::rgb32(end));
        painter.rect_stroke(chip, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    }
    let [from, to] = ends;
    let stops: Vec<Color32> = (0..=GRADIENT_STEPS).map(|k| super::rgb32(model::mix(from, to, k as f32 / GRADIENT_STEPS as f32))).collect();
    track(painter, rect, &stops, 1.0, at, t);
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
