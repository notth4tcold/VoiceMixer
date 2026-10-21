//! Custom controls: knobs, faders with meters, 2-D panels and toggle buttons.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Rect, Response, Sense, Shape, Stroke, Ui,
};
use std::f32::consts::PI;
use voicemixer_core::dsp::lin_to_db;
use voicemixer_core::{GAIN_MAX_DB, GAIN_MIN_DB, LIMIT_OFF_DB};

pub mod color {
    use eframe::egui::Color32;
    pub const BG: Color32 = Color32::from_rgb(0x15, 0x17, 0x1c);
    pub const PANEL: Color32 = Color32::from_rgb(0x1f, 0x23, 0x2b);
    pub const PANEL_HI: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3a);
    pub const TRACK: Color32 = Color32::from_rgb(0x0f, 0x11, 0x15);
    pub const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdd, 0xe6);
    pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x93, 0xa3);
    pub const HW: Color32 = Color32::from_rgb(0x2b, 0xb3, 0xa3);
    pub const VIRT: Color32 = Color32::from_rgb(0xe0, 0xa4, 0x3a);
    pub const BUS_A: Color32 = Color32::from_rgb(0x4c, 0xaf, 0x6a);
    pub const BUS_B: Color32 = Color32::from_rgb(0x4a, 0x8f, 0xe0);
    pub const MUTE: Color32 = Color32::from_rgb(0xd9, 0x53, 0x4f);
    pub const SOLO: Color32 = Color32::from_rgb(0xe6, 0xc3, 0x4a);
    pub const MONO: Color32 = Color32::from_rgb(0x9b, 0x7b, 0xe0);
    pub const REC: Color32 = Color32::from_rgb(0xf0, 0x40, 0x40);
    pub const OFF: Color32 = Color32::from_rgb(0x33, 0x38, 0x44);
    pub const METER_LO: Color32 = Color32::from_rgb(0x3d, 0xc4, 0x6b);
    pub const METER_MID: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x3a);
    pub const METER_HI: Color32 = Color32::from_rgb(0xec, 0x4b, 0x3c);

    pub fn bus(i: usize) -> Color32 {
        if i < voicemixer_core::NUM_HW_BUSES {
            BUS_A
        } else {
            BUS_B
        }
    }
}

/// Button that shows an on/off state with a fill colour. Returns the click response.
pub fn toggle(
    ui: &mut Ui,
    on: &mut bool,
    text: &str,
    on_color: Color32,
    size: egui::Vec2,
) -> Response {
    let fill = if *on { on_color } else { color::OFF };
    let fg = if *on { Color32::BLACK } else { color::TEXT };
    let mut resp = ui.add_sized(
        size,
        egui::Button::new(egui::RichText::new(text).color(fg).size(11.0).strong()).fill(fill),
    );
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// Rotary knob. Drag vertically, scroll, or double-click to reset.
pub fn knob(
    ui: &mut Ui,
    value: &mut f32,
    range: (f32, f32),
    default: f32,
    label: &str,
    accent: Color32,
    fmt: impl Fn(f32) -> String,
) -> Response {
    let (min, max) = range;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 54.0), Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        *value = (*value + (-d.y + d.x * 0.5) * (max - min) / 160.0).clamp(min, max);
        resp.mark_changed();
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            *value = (*value + scroll.signum() * (max - min) / 40.0).clamp(min, max);
            resp.mark_changed();
        }
    }
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    }
    let painter = ui.painter_at(rect.expand(2.0));
    let center = pos2(rect.center().x, rect.top() + 19.0);
    let r = 14.0;
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
        let steps = 24;
        (0..=steps)
            .map(|k| {
                let a = from + (to - from) * k as f32 / steps as f32;
                center + vec2(a.cos(), a.sin()) * r
            })
            .collect()
    };
    painter.circle_filled(center, r - 3.0, color::PANEL_HI);
    painter.add(Shape::line(
        arc(start, start + sweep),
        Stroke::new(3.0_f32, color::TRACK),
    ));
    // Bipolar knobs light up from the centre.
    let zero_t = if min < 0.0 && max > 0.0 {
        -min / (max - min)
    } else {
        0.0
    };
    let (a0, a1) = if t >= zero_t {
        (zero_t, t)
    } else {
        (t, zero_t)
    };
    if a1 - a0 > 0.001 {
        painter.add(Shape::line(
            arc(start + sweep * a0, start + sweep * a1),
            Stroke::new(3.0_f32, accent),
        ));
    }
    let a = start + sweep * t;
    painter.line_segment(
        [
            center + vec2(a.cos(), a.sin()) * 4.0,
            center + vec2(a.cos(), a.sin()) * (r - 4.0),
        ],
        Stroke::new(2.0_f32, color::TEXT),
    );
    let text = if resp.hovered() || resp.dragged() {
        fmt(*value)
    } else {
        label.to_string()
    };
    painter.text(
        pos2(rect.center().x, rect.bottom() - 7.0),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(10.0),
        color::DIM,
    );
    resp
}

fn db_to_t(db: f32) -> f32 {
    ((db - GAIN_MIN_DB) / (GAIN_MAX_DB - GAIN_MIN_DB)).clamp(0.0, 1.0)
}

fn t_to_db(t: f32) -> f32 {
    GAIN_MIN_DB + t.clamp(0.0, 1.0) * (GAIN_MAX_DB - GAIN_MIN_DB)
}

fn meter_color(db: f32) -> Color32 {
    if db > 0.0 {
        color::METER_HI
    } else if db > -12.0 {
        color::METER_MID
    } else {
        color::METER_LO
    }
}

pub struct FaderResponse {
    pub fader: Response,
    pub meter: Response,
}

/// Vertical gain fader with per-channel meters on its left. Dragging the meter
/// sets the limiter ceiling; double-clicking the meter disables the limiter.
pub fn vfader(
    ui: &mut Ui,
    gain_db: &mut f32,
    levels: &[f32],
    limit_db: Option<&mut f32>,
    height: f32,
    accent: Color32,
) -> FaderResponse {
    let n = levels.len().max(1);
    let meter_w = if n > 2 { 2.0 * n as f32 + 6.0 } else { 14.0 };
    let (meter_rect, mut meter) =
        ui.allocate_exact_size(vec2(meter_w, height), Sense::click_and_drag());
    let (rect, mut fader) = ui.allocate_exact_size(vec2(26.0, height), Sense::click_and_drag());
    let inner = |r: Rect| r.shrink2(vec2(0.0, 6.0));

    // Fader.
    let track = inner(rect);
    if fader.dragged() {
        let dy = fader.drag_delta().y;
        let fine = ui.input(|i| i.modifiers.shift);
        let k = if fine { 0.1 } else { 1.0 };
        *gain_db = t_to_db(db_to_t(*gain_db) - k * dy / track.height());
        fader.mark_changed();
    }
    if fader.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            *gain_db = (*gain_db + scroll.signum()).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
            fader.mark_changed();
        }
    }
    if fader.double_clicked() {
        *gain_db = 0.0;
        fader.mark_changed();
    }
    let painter = ui.painter();
    let groove = Rect::from_center_size(track.center(), vec2(4.0, track.height()));
    painter.rect_filled(groove, 2.0, color::TRACK);
    let zero_y = track.bottom() - db_to_t(0.0) * track.height();
    painter.line_segment(
        [
            pos2(track.left() + 3.0, zero_y),
            pos2(track.right() - 3.0, zero_y),
        ],
        Stroke::new(1.0_f32, color::DIM),
    );
    let y = track.bottom() - db_to_t(*gain_db) * track.height();
    let fill_rect = Rect::from_min_max(pos2(groove.left(), y), groove.max);
    painter.rect_filled(fill_rect, 2.0, accent.gamma_multiply(0.6));
    let cap = Rect::from_center_size(pos2(track.center().x, y), vec2(22.0, 12.0));
    painter.rect_filled(cap, 3.0, if fader.dragged() { accent } else { color::TEXT });
    painter.line_segment(
        [pos2(cap.left() + 3.0, y), pos2(cap.right() - 3.0, y)],
        Stroke::new(1.0_f32, Color32::BLACK),
    );

    // Meters.
    let mtrack = inner(meter_rect);
    painter.rect_filled(mtrack, 2.0, color::TRACK);
    let bar_w = (mtrack.width() - 2.0) / n as f32;
    for (c, &lv) in levels.iter().enumerate() {
        let db = lin_to_db(lv);
        let t = db_to_t(db);
        if t > 0.0 {
            let x0 = mtrack.left() + 1.0 + c as f32 * bar_w;
            let r = Rect::from_min_max(
                pos2(x0, mtrack.bottom() - t * mtrack.height()),
                pos2(x0 + bar_w - 1.0, mtrack.bottom()),
            );
            painter.rect_filled(r, 0.0, meter_color(db));
        }
    }
    if let Some(limit) = limit_db {
        if meter.dragged() || meter.clicked() {
            if let Some(pos) = meter.interact_pointer_pos() {
                let t = (mtrack.bottom() - pos.y) / mtrack.height();
                *limit = t_to_db(t).clamp(-40.0, LIMIT_OFF_DB);
                meter.mark_changed();
            }
        }
        if meter.double_clicked() {
            *limit = LIMIT_OFF_DB;
            meter.mark_changed();
        }
        if *limit < LIMIT_OFF_DB {
            let ly = mtrack.bottom() - db_to_t(*limit) * mtrack.height();
            painter.line_segment(
                [
                    pos2(mtrack.left() - 2.0, ly),
                    pos2(mtrack.right() + 2.0, ly),
                ],
                Stroke::new(2.0_f32, color::METER_HI),
            );
        }
    }
    FaderResponse { fader, meter }
}

/// Horizontal fader used by the bus section, with up to 8 meter bars under it.
pub fn hfader(
    ui: &mut Ui,
    gain_db: &mut f32,
    levels: &[f32],
    width: f32,
    accent: Color32,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    let track = Rect::from_min_max(
        rect.min + vec2(6.0, 2.0),
        pos2(rect.right() - 6.0, rect.top() + 14.0),
    );
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let k = if fine { 0.1 } else { 1.0 };
        *gain_db = t_to_db(db_to_t(*gain_db) + k * resp.drag_delta().x / track.width());
        resp.mark_changed();
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            *gain_db = (*gain_db + scroll.signum()).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
            resp.mark_changed();
        }
    }
    if resp.double_clicked() {
        *gain_db = 0.0;
        resp.mark_changed();
    }
    let painter = ui.painter();
    let groove = Rect::from_center_size(track.center(), vec2(track.width(), 4.0));
    painter.rect_filled(groove, 2.0, color::TRACK);
    let x = track.left() + db_to_t(*gain_db) * track.width();
    painter.rect_filled(
        Rect::from_min_max(groove.min, pos2(x, groove.bottom())),
        2.0,
        accent.gamma_multiply(0.6),
    );
    let zero_x = track.left() + db_to_t(0.0) * track.width();
    painter.line_segment(
        [pos2(zero_x, track.top()), pos2(zero_x, track.bottom())],
        Stroke::new(1.0_f32, color::DIM),
    );
    let cap = Rect::from_center_size(pos2(x, track.center().y), vec2(12.0, 14.0));
    painter.rect_filled(cap, 3.0, if resp.dragged() { accent } else { color::TEXT });

    let mrect = Rect::from_min_max(
        pos2(track.left(), rect.top() + 17.0),
        pos2(track.right(), rect.bottom()),
    );
    painter.rect_filled(mrect, 1.0, color::TRACK);
    let active = levels
        .iter()
        .rposition(|&v| v > 1e-5)
        .map_or(2, |i| (i + 1).max(2));
    let rows = if active > 2 { levels.len() } else { 2 };
    let bar_h = mrect.height() / rows as f32;
    for (c, &lv) in levels.iter().take(rows).enumerate() {
        let db = lin_to_db(lv);
        let t = db_to_t(db);
        if t > 0.0 {
            let y0 = mrect.top() + c as f32 * bar_h;
            let r = Rect::from_min_max(
                pos2(mrect.left(), y0),
                pos2(mrect.left() + t * mrect.width(), y0 + bar_h - 0.5),
            );
            painter.rect_filled(r, 0.0, meter_color(db));
        }
    }
    resp
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    Color,
    Position,
    /// Position panel of the 7.1 virtual strips: vertical axis is front/rear.
    Surround,
    Modulation,
}

/// 2-D panel (colour, position, modulation). Drag the dot; double-click resets.
pub fn pad(
    ui: &mut Ui,
    x: &mut f32,
    y: &mut f32,
    size: egui::Vec2,
    kind: PadKind,
    accent: Color32,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *x = ((pos.x - rect.center().x) / (rect.width() * 0.5)).clamp(-1.0, 1.0);
            *y = ((rect.center().y - pos.y) / (rect.height() * 0.5)).clamp(-1.0, 1.0);
            resp.mark_changed();
        }
    }
    if resp.double_clicked() {
        *x = 0.0;
        *y = 0.0;
        resp.mark_changed();
    }
    let painter = ui.painter_at(rect);
    let bg = match kind {
        PadKind::Color => Color32::from_rgb(0x1d, 0x26, 0x2e),
        PadKind::Position | PadKind::Surround => Color32::from_rgb(0x1d, 0x22, 0x30),
        PadKind::Modulation => Color32::from_rgb(0x26, 0x1f, 0x30),
    };
    painter.rect_filled(rect, 4.0, bg);
    let grid = Stroke::new(1.0_f32, color::PANEL_HI);
    painter.line_segment([rect.center_top(), rect.center_bottom()], grid);
    painter.line_segment([rect.left_center(), rect.right_center()], grid);
    // Axis labels at the middle of each edge: (left, right, top, bottom).
    let (left, right, top, bottom) = match kind {
        PadKind::Color => ("dark", "bright", "presence", "warm"),
        PadKind::Position => ("L", "R", "wide", "narrow"),
        PadKind::Surround => ("L", "R", "front", "rear"),
        PadKind::Modulation => ("slow", "fast", "chorus", "off"),
    };
    let f = FontId::proportional(9.0);
    painter.text(
        rect.left_center() + vec2(3.0, 0.0),
        Align2::LEFT_CENTER,
        left,
        f.clone(),
        color::DIM,
    );
    painter.text(
        rect.right_center() + vec2(-3.0, 0.0),
        Align2::RIGHT_CENTER,
        right,
        f.clone(),
        color::DIM,
    );
    painter.text(
        rect.center_top() + vec2(0.0, 2.0),
        Align2::CENTER_TOP,
        top,
        f.clone(),
        color::DIM,
    );
    painter.text(
        rect.center_bottom() + vec2(0.0, -2.0),
        Align2::CENTER_BOTTOM,
        bottom,
        f,
        color::DIM,
    );
    let dot = pos2(
        rect.center().x + *x * rect.width() * 0.5,
        rect.center().y - *y * rect.height() * 0.5,
    );
    let active = x.abs() > 0.001 || y.abs() > 0.001;
    painter.circle_filled(dot, 5.0, if active { accent } else { color::DIM });
    painter.circle_stroke(dot, 7.0, Stroke::new(1.0_f32, accent.gamma_multiply(0.5)));
    resp
}

/// Small coloured status box like "AUDIO" or "REMOTE 2".
pub fn chip(ui: &mut Ui, text: &str, on: bool, on_color: Color32) -> Response {
    let fill = if on { on_color } else { color::OFF };
    let fg = if on { Color32::BLACK } else { color::DIM };
    egui::Frame::none()
        .fill(fill)
        .rounding(3.0)
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(fg).size(10.5).strong())
        })
        .response
}

pub fn fmt_db(v: f32) -> String {
    format!("{v:+.1} dB")
}
