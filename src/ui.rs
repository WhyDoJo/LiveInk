use crate::drawing::{COLORS, Document, Tool};
use egui::{Align2, Color32, Context, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

const PANEL_WIDTH: f32 = 362.;
const PANEL_HEIGHT: f32 = 44.;
const BG: Color32 = Color32::from_rgb(20, 19, 23);
const BORDER: Color32 = Color32::from_rgb(43, 40, 49);
const TEXT: Color32 = Color32::from_rgb(191, 187, 199);
const MUTED: Color32 = Color32::from_rgb(112, 106, 125);
const ACCENT: Color32 = Color32::from_rgb(226, 215, 255);
const SELECTED: Color32 = Color32::from_rgb(67, 39, 126);
const HOVER: Color32 = Color32::from_rgb(40, 36, 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popover {
    Color,
    Width,
    More,
}

#[derive(Default)]
pub struct UiState {
    pub tool: Tool,
    pub color: usize,
    pub width_index: usize,
    pub passthrough: bool,
    pub help: bool,
    pub popover: Option<Popover>,
}

impl UiState {
    pub fn width(&self) -> f32 {
        [3., 5., 9.][self.width_index]
    }

    fn toggle_popover(&mut self, popover: Popover) {
        self.popover = if self.popover == Some(popover) {
            None
        } else {
            Some(popover)
        };
        self.help = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Undo,
    Redo,
    Clear,
    ToggleInput,
    Hide,
    Quit,
}

pub fn panel_rect(size: Vec2) -> Rect {
    let scale = ((size.x - 24.) / PANEL_WIDTH).clamp(0.1, 1.);
    Rect::from_min_size(
        pos2((size.x - PANEL_WIDTH * scale) * 0.5, 18.),
        vec2(PANEL_WIDTH, PANEL_HEIGHT) * scale,
    )
}

pub fn help_rect(size: Vec2) -> Rect {
    let panel = panel_rect(size);
    let width = (size.x - 24.).clamp(0., 380.);
    Rect::from_min_size(
        pos2((size.x - width) * 0.5, panel.bottom() + 10.),
        vec2(width, 270.),
    )
}

fn popover_rect(size: Vec2, popover: Popover) -> Rect {
    let panel = panel_rect(size);
    let width = (size.x - 24.).clamp(0., 212.);
    Rect::from_min_size(
        pos2((panel.right() - width).max(12.), panel.bottom() + 8.),
        vec2(width, if popover == Popover::More { 234. } else { 88. }),
    )
}

// Shared by rendering, cursor hit testing, and the Wayland click-through region.
// Popovers must remain clickable even when the desktop owns the rest of the input.
pub fn input_rects(size: Vec2, state: &UiState) -> [Option<Rect>; 3] {
    [
        Some(panel_rect(size).expand(3.)),
        state.popover.map(|p| popover_rect(size, p)),
        state.help.then(|| help_rect(size)),
    ]
}

pub fn contains_ui(size: Vec2, point: Pos2, state: &UiState) -> bool {
    input_rects(size, state)
        .into_iter()
        .flatten()
        .any(|r| r.contains(point))
}

pub fn draw(ctx: &Context, size: Vec2, state: &mut UiState, doc: &Document) -> Vec<Action> {
    let mut actions = Vec::new();
    let rect = panel_rect(size);
    let scale = rect.width() / PANEL_WIDTH;

    if ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !contains_ui(size, p, state))
    }) {
        state.popover = None;
    }

    egui::Area::new(Id::new("toolbar"))
        .fixed_pos(rect.min)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let p = ui.painter().clone();
            surface(&p, rect, 10.);
            let mut x = rect.left() + 6. * scale;
            let y = rect.center().y;

            let (response, r) = button(
                ui,
                &mut x,
                y,
                scale,
                "Interact with desktop · Tab",
                state.passthrough,
            );
            icon(
                &p,
                r.center(),
                Icon::Pointer,
                tint(state.passthrough),
                scale,
            );
            if response.clicked() {
                state.popover = None;
                actions.push(Action::ToggleInput);
            }
            divider(&p, &mut x, y, scale);

            // Shapes first, like Excalidraw; the document's tool order is unchanged.
            for (index, key) in [(3, "R"), (4, "O"), (2, "A"), (1, "L"), (0, "P"), (5, "E")] {
                let tool = Tool::ALL[index];
                let selected = state.tool == tool && !state.passthrough;
                let (response, r) = button(ui, &mut x, y, scale, tool.label(), selected);
                tool_icon(
                    &p,
                    r.center() - vec2(1., 1.) * scale,
                    tool,
                    tint(selected),
                    scale,
                );
                p.text(
                    r.right_bottom() - vec2(3., 2.) * scale,
                    Align2::RIGHT_BOTTOM,
                    key,
                    FontId::proportional(6.5 * scale),
                    if selected {
                        ACCENT.gamma_multiply(0.6)
                    } else {
                        MUTED
                    },
                );
                if response.clicked() {
                    state.tool = tool;
                    state.popover = None;
                    if state.passthrough {
                        actions.push(Action::ToggleInput);
                    }
                }
            }
            divider(&p, &mut x, y, scale);

            let (response, r) = button(
                ui,
                &mut x,
                y,
                scale,
                "Stroke color · 1–6",
                state.popover == Some(Popover::Color),
            );
            p.circle_stroke(
                r.center(),
                7. * scale,
                Stroke::new(1.2 * scale, COLORS[state.color]),
            );
            p.circle_filled(r.center(), 4.7 * scale, COLORS[state.color]);
            if response.clicked() {
                state.toggle_popover(Popover::Color);
            }

            let (response, r) = button(
                ui,
                &mut x,
                y,
                scale,
                "Stroke width · [ / ]",
                state.popover == Some(Popover::Width),
            );
            for (i, width) in [1., 2., 3.5].into_iter().enumerate() {
                let cy = r.center().y + (i as f32 - 1.) * 5. * scale;
                p.line_segment(
                    [
                        pos2(r.center().x - 6. * scale, cy),
                        pos2(r.center().x + 6. * scale, cy),
                    ],
                    Stroke::new(
                        width * scale,
                        if i == state.width_index {
                            ACCENT
                        } else {
                            MUTED
                        },
                    ),
                );
            }
            if response.clicked() {
                state.toggle_popover(Popover::Width);
            }
            divider(&p, &mut x, y, scale);

            let (response, r) = button(
                ui,
                &mut x,
                y,
                scale,
                "More actions",
                state.popover == Some(Popover::More),
            );
            for offset in [-5., 0., 5.] {
                p.circle_filled(r.center() + vec2(offset, 0.) * scale, 1.25 * scale, TEXT);
            }
            if response.clicked() {
                state.toggle_popover(Popover::More);
            }
        });

    if let Some(popover) = state.popover {
        let r = popover_rect(size, popover);
        egui::Area::new(Id::new("options"))
            .fixed_pos(r.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_size(r.size());
                let p = ui.painter().clone();
                surface(&p, r, 10.);
                match popover {
                    Popover::Color => {
                        caption(&p, r, "Stroke color");
                        for (i, color) in COLORS.iter().enumerate() {
                            let cell = Rect::from_min_size(
                                r.min + vec2(11. + i as f32 * 32., 37.),
                                vec2(30., 32.),
                            );
                            let response = ui
                                .interact(cell, Id::new(("color", i)), Sense::click())
                                .on_hover_text(format!("Color · {}", i + 1));
                            if response.hovered() {
                                p.rect_filled(cell, 6., HOVER);
                            }
                            if state.color == i {
                                p.rect_stroke(
                                    cell,
                                    6.,
                                    Stroke::new(1_f32, ACCENT),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            p.circle_filled(cell.center(), 7., *color);
                            if response.clicked() {
                                state.color = i;
                                state.popover = None;
                            }
                        }
                    }
                    Popover::Width => {
                        caption(&p, r, "Stroke width");
                        for (i, width) in [3_f32, 5., 9.].into_iter().enumerate() {
                            let cell = Rect::from_min_size(
                                r.min + vec2(12. + i as f32 * 64., 37.),
                                vec2(60., 32.),
                            );
                            let response = ui.interact(cell, Id::new(("width", i)), Sense::click());
                            if i == state.width_index || response.hovered() {
                                p.rect_filled(
                                    cell,
                                    6.,
                                    if i == state.width_index {
                                        SELECTED
                                    } else {
                                        HOVER
                                    },
                                );
                            }
                            p.line_segment(
                                [cell.center() - vec2(12., 0.), cell.center() + vec2(12., 0.)],
                                Stroke::new(width, TEXT),
                            );
                            if response.clicked() {
                                state.width_index = i;
                                state.popover = None;
                            }
                        }
                    }
                    Popover::More => {
                        for (i, (label, shortcut, glyph, action, enabled)) in [
                            (
                                "Undo",
                                "Ctrl Z",
                                Icon::Undo,
                                Some(Action::Undo),
                                doc.can_undo(),
                            ),
                            (
                                "Redo",
                                "Ctrl Shift Z",
                                Icon::Redo,
                                Some(Action::Redo),
                                doc.can_redo(),
                            ),
                            (
                                "Clear canvas",
                                "Ctrl Bksp",
                                Icon::Trash,
                                Some(Action::Clear),
                                !doc.strokes.is_empty(),
                            ),
                            ("Shortcuts", "F1", Icon::Help, None, true),
                            ("Hide", "Esc", Icon::Hide, Some(Action::Hide), true),
                            ("Quit", "Ctrl Q", Icon::Close, Some(Action::Quit), true),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let row = Rect::from_min_size(
                                r.min + vec2(6., 6. + i as f32 * 37.),
                                vec2(r.width() - 12., 37.),
                            );
                            let response = ui.interact(row, Id::new(("action", i)), Sense::click());
                            if response.hovered() && enabled {
                                p.rect_filled(row, 6., HOVER);
                            }
                            let color = if enabled {
                                TEXT
                            } else {
                                MUTED.gamma_multiply(0.5)
                            };
                            icon(&p, row.left_center() + vec2(17., 0.), glyph, color, 0.9);
                            p.text(
                                row.left_center() + vec2(36., 0.),
                                Align2::LEFT_CENTER,
                                label,
                                FontId::proportional(12.),
                                color,
                            );
                            p.text(
                                row.right_center() - vec2(10., 0.),
                                Align2::RIGHT_CENTER,
                                shortcut,
                                FontId::proportional(9.),
                                MUTED,
                            );
                            if response.clicked() && enabled {
                                state.popover = None;
                                if let Some(action) = action {
                                    actions.push(action);
                                } else {
                                    state.help = !state.help;
                                }
                            }
                        }
                    }
                }
            });
    }

    if state.help {
        let r = help_rect(size);
        egui::Area::new(Id::new("help"))
            .fixed_pos(r.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_size(r.size());
                let p = ui.painter();
                surface(p, r, 10.);
                p.text(
                    r.min + vec2(20., 20.),
                    Align2::LEFT_TOP,
                    "Keyboard shortcuts",
                    FontId::proportional(14.),
                    ACCENT,
                );
                for (i, (key, label)) in [
                    ("P / L / A", "Pen / line / arrow"),
                    ("R / O / E", "Rectangle / ellipse / eraser"),
                    ("1 – 6     [ / ]", "Color / stroke width"),
                    ("Ctrl Z / Shift Z", "Undo / redo"),
                    ("Shift + drag", "Snap lines / square / circle"),
                    ("Tab", "Interact with the live desktop"),
                    ("Esc / Ctrl Q", "Hide / quit"),
                ]
                .iter()
                .enumerate()
                {
                    let y = r.top() + 64. + i as f32 * 25.;
                    p.text(
                        pos2(r.left() + 20., y),
                        Align2::LEFT_CENTER,
                        *key,
                        FontId::monospace(11.),
                        MUTED,
                    );
                    p.text(
                        pos2(r.left() + 158., y),
                        Align2::LEFT_CENTER,
                        *label,
                        FontId::proportional(11.),
                        TEXT,
                    );
                }
            });
    }
    actions
}

fn tint(selected: bool) -> Color32 {
    if selected { ACCENT } else { TEXT }
}

fn surface(p: &egui::Painter, r: Rect, radius: f32) {
    p.rect_filled(
        r.translate(vec2(0., 2.)),
        radius + 1.,
        Color32::from_black_alpha(28),
    );
    p.rect_filled(r, radius, BG);
    p.rect_stroke(
        r.shrink(0.5),
        radius,
        Stroke::new(1_f32, BORDER),
        egui::StrokeKind::Inside,
    );
}

fn caption(p: &egui::Painter, r: Rect, text: &str) {
    p.text(
        r.min + vec2(14., 15.),
        Align2::LEFT_TOP,
        text,
        FontId::proportional(11.),
        MUTED,
    );
}

fn button(
    ui: &mut egui::Ui,
    x: &mut f32,
    y: f32,
    scale: f32,
    label: &str,
    selected: bool,
) -> (egui::Response, Rect) {
    let r = Rect::from_center_size(pos2(*x + 16. * scale, y), vec2(30., 30.) * scale);
    *x += 32. * scale;
    let response = ui
        .interact(r, Id::new(label), Sense::click())
        .on_hover_text(label);
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(r, 7., if selected { SELECTED } else { HOVER });
    }
    (response, r)
}

fn divider(p: &egui::Painter, x: &mut f32, y: f32, scale: f32) {
    let cx = *x + 5. * scale;
    p.line_segment(
        [pos2(cx, y - 9. * scale), pos2(cx, y + 9. * scale)],
        Stroke::new(1_f32, BORDER),
    );
    *x += 10. * scale;
}

fn tool_icon(p: &egui::Painter, c: Pos2, tool: Tool, color: Color32, scale: f32) {
    let point = |x, y| c + vec2(x, y) * scale;
    let line = |a, b| {
        p.line_segment([a, b], Stroke::new(1.25 * scale, color));
    };
    match tool {
        Tool::Pen => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(-6., 6.),
                    point(-5., 2.),
                    point(3., -6.),
                    point(6., -3.),
                    point(-2., 5.),
                ],
                Stroke::new(1.25 * scale, color),
            ));
            line(point(1., -4.), point(4., -1.));
        }
        Tool::Line => line(point(-6., 0.), point(6., 0.)),
        Tool::Arrow => {
            line(point(-7., 0.), point(7., 0.));
            line(point(3., -4.), point(7., 0.));
            line(point(7., 0.), point(3., 4.));
        }
        Tool::Rectangle => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(13., 12.) * scale),
                2.,
                Stroke::new(1.25 * scale, color),
                egui::StrokeKind::Inside,
            );
        }
        Tool::Ellipse => {
            p.circle_stroke(c, 6.5 * scale, Stroke::new(1.25 * scale, color));
        }
        Tool::Eraser => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(-6., 1.),
                    point(1., -6.),
                    point(6., -1.),
                    point(-1., 6.),
                    point(-3., 6.),
                ],
                Stroke::new(1.25 * scale, color),
            ));
            line(point(-3., -2.), point(2., 3.));
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Undo,
    Redo,
    Trash,
    Pointer,
    Help,
    Hide,
    Close,
}

fn icon(p: &egui::Painter, c: Pos2, glyph: Icon, color: Color32, scale: f32) {
    let point = |x, y| c + vec2(x, y) * scale;
    let line = |a, b| {
        p.line_segment([a, b], Stroke::new(1.25 * scale, color));
    };
    match glyph {
        Icon::Undo | Icon::Redo => {
            let flip = if matches!(glyph, Icon::Redo) { -1. } else { 1. };
            p.add(egui::Shape::line(
                [[-5., -3.], [1., -3.], [5., -1.], [5., 3.], [2., 5.]]
                    .iter()
                    .map(|v| point(v[0] * flip, v[1]))
                    .collect(),
                Stroke::new(1.25 * scale, color),
            ));
            line(point(-2. * flip, -6.), point(-5. * flip, -3.));
            line(point(-5. * flip, -3.), point(-2. * flip, 0.));
        }
        Icon::Trash => {
            line(point(-6., -4.), point(6., -4.));
            line(point(-2., -7.), point(2., -7.));
            line(point(-4., -1.), point(-3., 6.));
            line(point(-3., 6.), point(3., 6.));
            line(point(3., 6.), point(4., -1.));
        }
        Icon::Pointer => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(-5., -7.),
                    point(6., 0.),
                    point(1., 1.),
                    point(-1., 6.),
                ],
                Stroke::new(1.25 * scale, color),
            ));
            line(point(1., 1.), point(4., 6.));
        }
        Icon::Help => {
            p.text(
                c,
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(17. * scale),
                color,
            );
        }
        Icon::Hide => line(point(-6., 0.), point(6., 0.)),
        Icon::Close => {
            line(point(-4., -4.), point(4., 4.));
            line(point(-4., 4.), point(4., -4.));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_fits_small_and_large_screens() {
        for width in [320., 640., 800., 1280., 1920., 3840.] {
            let rect = panel_rect(vec2(width, 1080.));
            assert!(rect.left() >= 0. && rect.right() <= width);
            assert!(rect.width() <= 362. && rect.height() <= 44.);
        }
    }

    #[test]
    fn click_through_only_keeps_visible_controls_interactive() {
        let size = vec2(1920., 1080.);
        let mut state = UiState::default();
        assert!(contains_ui(size, panel_rect(size).center(), &state));
        assert!(!contains_ui(size, pos2(500., 500.), &state));
        assert!(!contains_ui(size, help_rect(size).center(), &state));
        state.help = true;
        assert!(contains_ui(size, help_rect(size).center(), &state));
        state.help = false;
        for popover in [Popover::Color, Popover::Width, Popover::More] {
            state.popover = Some(popover);
            assert!(contains_ui(
                size,
                popover_rect(size, popover).center(),
                &state
            ));
            assert!(!contains_ui(size, pos2(500., 500.), &state));
        }
    }

    #[test]
    fn popover_toggle_keeps_one_panel_open() {
        let mut state = UiState {
            help: true,
            ..Default::default()
        };
        state.toggle_popover(Popover::Color);
        assert!(!state.help);
        assert_eq!(state.popover, Some(Popover::Color));
        state.toggle_popover(Popover::More);
        assert_eq!(state.popover, Some(Popover::More));
        state.toggle_popover(Popover::More);
        assert_eq!(state.popover, None);
    }

    fn frame(
        ctx: &Context,
        state: &mut UiState,
        doc: &Document,
        events: Vec<egui::Event>,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1920., 1080.))),
                events,
                ..Default::default()
            },
            |ctx| {
                actions = draw(ctx, vec2(1920., 1080.), state, doc);
            },
        );
        actions
    }

    fn click(ctx: &Context, state: &mut UiState, doc: &Document, pos: Pos2) -> Vec<Action> {
        let mut actions = Vec::new();
        for pressed in [true, false] {
            actions.extend(frame(
                ctx,
                state,
                doc,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            ));
        }
        // Let egui settle new/removed Areas before the next interaction.
        frame(ctx, state, doc, Vec::new());
        actions
    }

    #[test]
    fn compact_controls_keep_tools_colors_and_widths_accessible() {
        let ctx = Context::default();
        let mut state = UiState::default();
        let doc = Document::default();
        frame(&ctx, &mut state, &doc, Vec::new());
        frame(&ctx, &mut state, &doc, Vec::new());
        let panel = panel_rect(vec2(1920., 1080.));
        for (offset, expected) in [
            (64., Tool::Rectangle),
            (96., Tool::Ellipse),
            (128., Tool::Arrow),
            (160., Tool::Line),
            (192., Tool::Pen),
            (224., Tool::Eraser),
        ] {
            click(
                &ctx,
                &mut state,
                &doc,
                pos2(panel.left() + offset, panel.center().y),
            );
            assert_eq!(state.tool, expected);
        }
        click(
            &ctx,
            &mut state,
            &doc,
            pos2(panel.left() + 266., panel.center().y),
        );
        assert_eq!(state.popover, Some(Popover::Color));
        let palette = popover_rect(vec2(1920., 1080.), Popover::Color);
        click(
            &ctx,
            &mut state,
            &doc,
            palette.min + vec2(11. + 32. * 3. + 15., 53.),
        );
        assert_eq!(state.color, 3);
        assert_eq!(state.popover, None);
        click(
            &ctx,
            &mut state,
            &doc,
            pos2(panel.left() + 298., panel.center().y),
        );
        assert_eq!(state.popover, Some(Popover::Width));
        let widths = popover_rect(vec2(1920., 1080.), Popover::Width);
        click(
            &ctx,
            &mut state,
            &doc,
            widths.min + vec2(12. + 64. * 2. + 30., 53.),
        );
        assert_eq!(state.width_index, 2);
        state.passthrough = true;
        let actions = click(
            &ctx,
            &mut state,
            &doc,
            pos2(panel.left() + 192., panel.center().y),
        );
        assert_eq!(state.tool, Tool::Pen);
        assert_eq!(actions, vec![Action::ToggleInput]);
    }

    #[test]
    fn overflow_preserves_actions_and_disabled_history() {
        let ctx = Context::default();
        let mut state = UiState::default();
        let mut doc = Document::default();
        frame(&ctx, &mut state, &doc, Vec::new());
        frame(&ctx, &mut state, &doc, Vec::new());
        let panel = panel_rect(vec2(1920., 1080.));
        let more_button = pos2(panel.left() + 340., panel.center().y);
        let menu = popover_rect(vec2(1920., 1080.), Popover::More);
        click(&ctx, &mut state, &doc, more_button);
        let undo_pos = menu.min + vec2(70., 24.5);
        assert!(click(&ctx, &mut state, &doc, undo_pos).is_empty());
        assert_eq!(state.popover, Some(Popover::More));
        doc.checkpoint();
        frame(&ctx, &mut state, &doc, Vec::new());
        assert_eq!(click(&ctx, &mut state, &doc, undo_pos), vec![Action::Undo]);
        for (row, action) in [(4., Action::Hide), (5., Action::Quit)] {
            click(&ctx, &mut state, &doc, more_button);
            assert_eq!(
                click(
                    &ctx,
                    &mut state,
                    &doc,
                    menu.min + vec2(70., 24.5 + row * 37.)
                ),
                vec![action]
            );
        }
        click(&ctx, &mut state, &doc, more_button);
        assert!(
            click(
                &ctx,
                &mut state,
                &doc,
                menu.min + vec2(70., 24.5 + 3. * 37.)
            )
            .is_empty()
        );
        assert!(state.help);
        assert_eq!(state.popover, None);
    }
}
