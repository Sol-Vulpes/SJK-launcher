//! SJK's look (`docs/sjk-ui.md` in SJK): the navy Space ground, holo blue for
//! line-work, gold for what is chosen and the one main action, ember for
//! leaving and harm; Rajdhani for titles and navigation, Exo 2 for the rest.
//! Content floats on the backdrop, a holo floor running to the horizon, with
//! thin lines for structure instead of boxes.

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Mesh, Painter, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextStyle, TextureHandle, Vec2,
};
use std::sync::Arc;

// SJK: navy ground, holo blue lines, gold for the chosen and the main action.
#[cfg(not(feature = "jof"))]
mod palette {
    use super::Color32;
    pub const SPACE: Color32 = Color32::from_rgb(0x06, 0x0A, 0x14);
    pub const RAISED: Color32 = Color32::from_rgb(0x0A, 0x11, 0x22);
    pub const SKY: Color32 = Color32::from_rgb(0x0C, 0x18, 0x30);
    pub const HOLO: Color32 = Color32::from_rgb(0xA8, 0xCF, 0xFF);
    pub const GOLD: Color32 = Color32::from_rgb(0xE8, 0xB8, 0x4A);
    pub const GOLD_BRIGHT: Color32 = Color32::from_rgb(0xFF, 0xD9, 0x7A);
    pub const TEXT: Color32 = Color32::from_rgb(0xE2, 0xE9, 0xF6);
    pub const MUTED: Color32 = Color32::from_rgb(0xA9, 0xB5, 0xCB);
}

// JoF: the clan logo's green on a near-black green ground, mint lines. The names
// stay SJK's (GOLD is the chosen and the main action, HOLO the line-work).
#[cfg(feature = "jof")]
mod palette {
    use super::Color32;
    pub const SPACE: Color32 = Color32::from_rgb(0x04, 0x0B, 0x07);
    pub const RAISED: Color32 = Color32::from_rgb(0x09, 0x16, 0x0E);
    pub const SKY: Color32 = Color32::from_rgb(0x0A, 0x24, 0x14);
    pub const HOLO: Color32 = Color32::from_rgb(0x9C, 0xE6, 0xB4);
    pub const GOLD: Color32 = Color32::from_rgb(0x22, 0xC8, 0x38);
    pub const GOLD_BRIGHT: Color32 = Color32::from_rgb(0x74, 0xF2, 0x86);
    pub const TEXT: Color32 = Color32::from_rgb(0xE3, 0xF3, 0xE7);
    pub const MUTED: Color32 = Color32::from_rgb(0xA2, 0xBC, 0xA9);
}

pub use palette::*;
pub const EMBER: Color32 = Color32::from_rgb(0xFF, 0x7A, 0x3D);
/// The scan's warnings: amber in both looks (the JoF one's gold is green).
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xD9, 0x7A);

fn with_alpha(colour: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
}

/// The chosen colour at a strength, for fills.
pub fn gold(alpha: u8) -> Color32 {
    with_alpha(GOLD, alpha)
}

pub fn ember(alpha: u8) -> Color32 {
    with_alpha(EMBER, alpha)
}

/// Holo at a strength, for lines.
pub fn holo(alpha: u8) -> Color32 {
    with_alpha(HOLO, alpha)
}

fn display_family() -> FontFamily {
    FontFamily::Name("display".into())
}

fn strong_family() -> FontFamily {
    FontFamily::Name("body-strong".into())
}

/// Rajdhani Bold: titles, navigation, numbers.
pub fn display(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(display_family()).size(size)
}

/// Exo 2 SemiBold.
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(strong_family()).size(size)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // Exo 2's tall ascent leaves its capitals sitting low in a row; lift them so
    // text is centred in buttons and beside checkboxes (measured on screen).
    for (name, bytes, lift) in [
        ("rajdhani-bold", &include_bytes!("../assets/fonts/Rajdhani-Bold.ttf")[..], 0.0),
        ("exo2", &include_bytes!("../assets/fonts/Exo2-Regular.ttf")[..], -0.085),
        ("exo2-semibold", &include_bytes!("../assets/fonts/Exo2-SemiBold.ttf")[..], -0.085),
    ] {
        let tweak = FontTweak { y_offset_factor: lift, ..Default::default() };
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes).tweak(tweak)));
    }
    // egui's own fonts stay behind ours for the symbols they lack (arrows, ticks).
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &str| [vec![first.to_owned()], fallback.clone()].concat();
    fonts.families.insert(FontFamily::Proportional, with("exo2"));
    fonts.families.insert(display_family(), with("rajdhani-bold"));
    fonts.families.insert(strong_family(), with("exo2-semibold"));
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = SPACE;
    visuals.window_fill = RAISED;
    visuals.extreme_bg_color = Color32::from_rgb(0x04, 0x07, 0x10);
    visuals.faint_bg_color = RAISED;
    visuals.hyperlink_color = GOLD_BRIGHT;
    visuals.selection.bg_fill = gold(70);
    visuals.selection.stroke = Stroke::new(1.0, GOLD_BRIGHT);
    visuals.window_stroke = Stroke::new(1.0, holo(90));
    visuals.window_corner_radius = 8.into();
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, holo(70));
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    for (state, fill, stroke) in [
        (&mut widgets.inactive, holo(12), holo(110)),
        (&mut widgets.hovered, holo(28), GOLD),
        (&mut widgets.active, gold(60), GOLD_BRIGHT),
        (&mut widgets.open, holo(28), GOLD),
    ] {
        state.weak_bg_fill = fill;
        state.bg_fill = RAISED;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.5, TEXT);
        // Small enough that a checkbox stays a square.
        state.corner_radius = 3.into();
        state.expansion = 0.0;
    }
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(12.5));
        style.text_styles.insert(TextStyle::Heading, FontId::new(24.0, display_family()));
    });
}

/// SJK's emblem, decoded once.
pub fn emblem(ctx: &egui::Context) -> Option<TextureHandle> {
    let image = image::load_from_memory_with_format(crate::brand::EMBLEM, image::ImageFormat::Png).ok()?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture("sjk-emblem", pixels, egui::TextureOptions::LINEAR))
}

fn vertical_gradient(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Where the horizon sits in a window of this rect.
pub fn horizon(rect: Rect) -> f32 {
    rect.top() + rect.height() * 0.59
}

/// The ground: sky fading to the horizon, then (with `floor`) a holo grid running
/// away from the viewer and SJK's emblem floating over it.
pub fn backdrop(painter: &Painter, rect: Rect, floor: bool, emblem: Option<&TextureHandle>) {
    painter.rect_filled(rect, 0.0, SPACE);
    let scale = rect.height() / 800.0;
    if !floor {
        vertical_gradient(painter, Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + 260.0 * scale)), RAISED, SPACE);
        return;
    }
    let horizon = horizon(rect);
    vertical_gradient(painter, Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), horizon)), SPACE, SKY);
    let depth = rect.bottom() - horizon;
    let centre = rect.center().x;
    // Rows: close together far away, wider as they come near.
    let rows = 14;
    for row in 1..=rows {
        let t = row as f32 / rows as f32;
        let y = horizon + depth * t.powf(1.9);
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, holo((30.0 + 60.0 * t) as u8)));
    }
    // Columns converge towards a vanishing point well above the horizon.
    let spacing = 92.0 * scale;
    for column in -30..=30 {
        let far = Pos2::new(centre + column as f32 * spacing * 0.32, horizon);
        let near = Pos2::new(centre + column as f32 * spacing * 1.7, rect.bottom());
        painter.line_segment([far, near], Stroke::new(1.0, holo(55)));
    }
    painter.line_segment([Pos2::new(rect.left(), horizon), Pos2::new(rect.right(), horizon)], Stroke::new(1.5 * scale.max(1.0), holo(205)));
    // The floor fades into the ground towards the viewer, under the dock.
    let fade = Rect::from_min_max(Pos2::new(rect.left(), horizon + 1.0), rect.right_bottom());
    let mut mesh = Mesh::default();
    let clear = with_alpha(SPACE, 0);
    let mid = Pos2::new(fade.right(), fade.top() + fade.height() * 0.8);
    mesh.colored_vertex(fade.left_top(), clear);
    mesh.colored_vertex(fade.right_top(), clear);
    mesh.colored_vertex(mid, SPACE);
    mesh.colored_vertex(Pos2::new(fade.left(), mid.y), SPACE);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    painter.rect_filled(Rect::from_min_max(Pos2::new(fade.left(), mid.y), fade.right_bottom()), 0.0, SPACE);
    if let Some(emblem) = emblem {
        let size = 420.0 * scale;
        let at = Pos2::new(rect.right() - 70.0 * scale - size, rect.top() + 96.0 * scale);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(emblem.id(), Rect::from_min_size(at, Vec2::splat(size)), uv, Color32::from_white_alpha(235));
    }
}

/// The screen's one main action: gold, or an outline while it cannot be pressed.
/// Drawn by hand: egui's button sets large text well below its middle.
pub fn main_button(ui: &mut egui::Ui, text: &str, size: Vec2, enabled: bool) -> Response {
    let colour = if enabled { SPACE } else { MUTED };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(size.y * 0.44, display_family()), colour);
    let size = Vec2::new(size.x.max(galley.size().x + size.y), size.y);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let (fill, stroke) = if !enabled {
        (Color32::TRANSPARENT, Stroke::new(1.0, holo(115)))
    } else if response.hovered() {
        (GOLD_BRIGHT, Stroke::NONE)
    } else {
        (GOLD, Stroke::NONE)
    };
    let painter = ui.painter();
    painter.rect(rect, 6.0, fill, stroke, StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, colour);
    if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

#[derive(Clone, Copy, PartialEq)]
pub enum WindowButton {
    Minimise,
    Maximise,
    Restore,
    Close,
}

/// The title bar's window controls, drawn as thin holo line-work.
pub fn window_button(ui: &mut egui::Ui, kind: WindowButton, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let hovered = response.hovered();
    if hovered {
        let fill = if kind == WindowButton::Close { ember(190) } else { holo(28) };
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let colour = match (hovered, kind) {
        (true, WindowButton::Close) => SPACE,
        (true, _) => GOLD_BRIGHT,
        _ => MUTED,
    };
    let stroke = Stroke::new(1.4, colour);
    let painter = ui.painter();
    let c = rect.center();
    let r = (size * 0.17).round();
    match kind {
        WindowButton::Minimise => {
            painter.line_segment([Pos2::new(c.x - r, c.y), Pos2::new(c.x + r, c.y)], stroke);
        }
        WindowButton::Maximise => {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * r)), 1.0, stroke, StrokeKind::Middle);
        }
        WindowButton::Restore => {
            let front = Rect::from_center_size(c + Vec2::new(-2.0, 2.0), Vec2::splat(2.0 * r - 2.0));
            painter.rect_stroke(front, 1.0, stroke, StrokeKind::Middle);
            let back = front.translate(Vec2::new(4.0, -4.0));
            painter.line_segment([Pos2::new(back.left(), front.top()), back.left_top()], stroke);
            painter.line_segment([back.left_top(), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom()], stroke);
            painter.line_segment([back.right_bottom(), Pos2::new(front.right(), back.bottom())], stroke);
        }
        WindowButton::Close => {
            painter.line_segment([c - Vec2::splat(r), c + Vec2::splat(r)], stroke);
            painter.line_segment([c + Vec2::new(-r, r), c + Vec2::new(r, -r)], stroke);
        }
    }
    response
}

/// A navigation tab: gold and underlined when chosen.
pub fn tab(ui: &mut egui::Ui, text: &str, size: f32, chosen: bool) -> Response {
    let colour = if chosen { GOLD_BRIGHT } else { MUTED };
    let response = ui.add(egui::Label::new(display(text, size).color(colour)).sense(Sense::click()));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let line = if chosen {
        Some(GOLD)
    } else if response.hovered() {
        Some(holo(120))
    } else {
        None
    };
    if let Some(colour) = line {
        let rect = response.rect;
        let y = rect.bottom() + 5.0;
        ui.painter().line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(2.0, colour));
    }
    response
}

/// A thin progress line, holo on a faint track.
pub fn progress_line(ui: &mut egui::Ui, width: f32, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 3.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, holo(60));
    let mut done = rect;
    done.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    ui.painter().rect_filled(done, 0.0, HOLO);
}

/// A thin holo rule across the available width.
pub fn rule(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, holo(80));
}
