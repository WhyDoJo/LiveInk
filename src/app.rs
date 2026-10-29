use crate::{
    drawing::{COLORS, Document, Instance, Stroke, Tool},
    ipc::{self, Command},
    renderer::Renderer,
    ui::{self, Action, UiState},
};
use anyhow::{Context, Result, bail};
use calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use egui::{Event, Pos2, Vec2, pos2, vec2};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            BTN_LEFT, BTN_RIGHT, CursorIcon, PointerEvent, PointerEventKind, PointerHandler,
            ThemeSpec, ThemedPointer,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler},
};
use std::{
    io,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};

pub fn run(requested_output: Option<String>) -> Result<()> {
    let (_guard, socket) = ipc::Guard::bind()?;
    let conn = Connection::connect_to_env().context("Cannot connect to Wayland")?;
    let (globals, mut queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("Your compositor must support wlr-layer-shell (Hyprland does)")?;
    let mut app = App {
        renderer: None,
        layer: None,
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm: Shm::bind(&globals, &qh)?,
        compositor,
        keyboard: None,
        pointer: None,
        cursor: None,
        size: vec2(1920., 1080.),
        scale: 1,
        configured: false,
        visible: true,
        dirty: true,
        frame_pending: false,
        exit: false,
        document: Document::default(),
        active: None,
        erasing: false,
        erase_checkpoint: false,
        ui: UiState::default(),
        egui: egui::Context::default(),
        events: Vec::new(),
        modifiers: egui::Modifiers::default(),
        pointer_pos: None,
        start: Instant::now(),
        next_repaint: None,
        ink: Vec::new(),
        conn,
        qh,
    };
    app.egui.style_mut(|s| {
        s.animation_time = 0.;
        s.visuals = egui::Visuals::dark();
    });
    // Receive names and seat capabilities before choosing the output.
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;
    let desired = requested_output.clone().or_else(focused_monitor);
    let output = desired.as_ref().and_then(|name| {
        app.output_state
            .outputs()
            .find(|o| app.output_state.info(o).and_then(|i| i.name).as_ref() == Some(name))
    });
    if let Some(name) = &requested_output
        && output.is_none()
    {
        let names: Vec<_> = app
            .output_state
            .outputs()
            .filter_map(|o| app.output_state.info(&o).and_then(|i| i.name))
            .collect();
        bail!(
            "Monitor {:?} not found. Available: {}",
            name,
            names.join(", ")
        );
    }
    eprintln!(
        "LivePaint output: {}",
        desired.as_deref().unwrap_or("compositor default")
    );
    let surface = app.compositor.create_surface(&app.qh);
    let layer = layer_shell.create_layer_surface(
        &app.qh,
        surface,
        Layer::Overlay,
        Some("livepaint"),
        output.as_ref(),
    );
    app.layer = Some(layer);
    app.configure_layer();
    app.renderer = Some(Renderer::new(&app.conn, app.surface())?);
    app.layer.as_ref().unwrap().commit();

    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    WaylandSource::new(app.conn.clone(), queue).insert(event_loop.handle())?;
    event_loop
        .handle()
        .insert_source(
            Generic::new(socket, Interest::READ, Mode::Level),
            |_, socket, app| {
                let mut bytes = [0_u8; 64];
                loop {
                    match socket.recv(&mut bytes) {
                        Ok(len) => {
                            if let Ok(s) = std::str::from_utf8(&bytes[..len])
                                && let Some(command) = Command::parse(s)
                            {
                                app.command(command);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            eprintln!("Control socket: {e}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("Cannot register control socket: {e}"))?;

    eprintln!("LivePaint ready. Esc: hide · Tab: click-through · Ctrl+Q: quit");
    while !app.exit {
        if app.next_repaint.is_some_and(|at| at <= Instant::now()) {
            app.next_repaint = None;
            app.dirty = true;
        }
        if app.visible && app.configured && app.dirty && !app.frame_pending {
            app.render()?;
        }
        app.conn.flush()?;
        // Sleep indefinitely when idle. Frame callbacks pace drawing to the monitor;
        // egui may request a single timed wakeup for tooltip/layout changes.
        let timeout = if app.visible && !app.frame_pending {
            app.next_repaint
                .map(|at| at.saturating_duration_since(Instant::now()))
        } else {
            None
        };
        if !app.exit {
            event_loop.dispatch(timeout, &mut app)?;
        }
    }
    Ok(())
}

fn focused_monitor() -> Option<String> {
    let result = std::process::Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .ok()?;
    if !result.status.success() {
        return None;
    }
    let monitors: serde_json::Value = serde_json::from_slice(&result.stdout).ok()?;
    monitors
        .as_array()?
        .iter()
        .find(|m| m["focused"].as_bool() == Some(true))?["name"]
        .as_str()
        .map(str::to_owned)
}

struct App {
    renderer: Option<Renderer>,
    layer: Option<LayerSurface>,
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    compositor: CompositorState,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer>,
    cursor: Option<CursorIcon>,
    size: Vec2,
    scale: i32,
    configured: bool,
    visible: bool,
    dirty: bool,
    frame_pending: bool,
    exit: bool,
    document: Document,
    active: Option<Stroke>,
    erasing: bool,
    erase_checkpoint: bool,
    ui: UiState,
    egui: egui::Context,
    events: Vec<Event>,
    modifiers: egui::Modifiers,
    pointer_pos: Option<Pos2>,
    start: Instant,
    next_repaint: Option<Instant>,
    ink: Vec<Instance>,
    // Must outlive Renderer and LayerSurface.
    conn: Connection,
    qh: QueueHandle<Self>,
}

impl App {
    fn surface(&self) -> &wl_surface::WlSurface {
        self.layer.as_ref().unwrap().wl_surface()
    }

    fn configure_layer(&self) {
        let layer = self.layer.as_ref().unwrap();
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        // -1 means cover panels too, without reserving any desktop space.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(if self.visible && !self.ui.passthrough {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        });
    }

    fn sync_input(&mut self) {
        if self.layer.is_none() {
            return;
        }
        self.configure_layer();
        if self.ui.passthrough || !self.visible {
            if let Ok(region) = Region::new(&self.compositor) {
                if self.visible {
                    for r in ui::input_rects(self.size, &self.ui).into_iter().flatten() {
                        region.add(
                            r.left().floor() as i32,
                            r.top().floor() as i32,
                            r.width().ceil() as i32,
                            r.height().ceil() as i32,
                        );
                    }
                }
                self.surface().set_input_region(Some(region.wl_region()));
            }
        } else {
            self.surface().set_input_region(None);
        }
        self.layer.as_ref().unwrap().commit();
        self.cursor = None;
        self.update_cursor();
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Toggle => self.set_visible(!self.visible),
            Command::Show => self.set_visible(true),
            Command::Hide => self.set_visible(false),
            Command::Passthrough => {
                if !self.visible {
                    self.set_visible(true);
                }
                self.finish_gesture();
                self.ui.passthrough = !self.ui.passthrough;
                self.modifiers = egui::Modifiers::default();
                self.sync_input();
            }
            Command::Clear => {
                self.finish_gesture();
                self.document.clear();
            }
            Command::Undo => {
                self.finish_gesture();
                self.document.undo();
            }
            Command::Redo => {
                self.finish_gesture();
                self.document.redo();
            }
            Command::Quit => self.exit = true,
        }
        self.dirty = true;
    }

    fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.finish_gesture();
        self.visible = visible;
        self.events.clear();
        self.events.push(Event::PointerGone);
        self.pointer_pos = None;
        self.modifiers = egui::Modifiers::default();
        self.frame_pending = false;
        self.configured = false;
        self.next_repaint = None;
        if visible {
            // Remap after the initial empty commit/configure handshake.
            self.ui.passthrough = false;
            self.configure_layer();
            self.surface().set_input_region(None);
            self.layer.as_ref().unwrap().commit();
        } else {
            self.sync_input();
            self.surface().attach(None, 0, 0);
            self.layer.as_ref().unwrap().commit();
        }
        self.dirty = true;
    }

    fn finish_gesture(&mut self) {
        if let Some(stroke) = self.active.take() {
            self.document.checkpoint();
            self.document.strokes.push(std::sync::Arc::new(stroke));
        }
        self.erasing = false;
        self.erase_checkpoint = false;
    }

    fn erase_at(&mut self, pos: Pos2) {
        let radius = 12. + self.ui.width();
        if self.document.strokes.iter().any(|s| s.hit(pos, radius)) {
            if !self.erase_checkpoint {
                self.document.checkpoint();
                self.erase_checkpoint = true;
            }
            self.document.strokes.retain(|s| !s.hit(pos, radius));
        }
    }

    fn draw_to(&mut self, mut pos: Pos2) {
        if self.erasing {
            self.erase_at(pos);
            self.dirty = true;
            return;
        }
        let Some(stroke) = &mut self.active else {
            return;
        };
        let a = stroke.points[0];
        if self.modifiers.shift && stroke.tool != Tool::Pen {
            let d = pos - a;
            if matches!(stroke.tool, Tool::Rectangle | Tool::Ellipse) {
                let side = d.x.abs().max(d.y.abs());
                pos = a + vec2(
                    if d.x < 0. { -side } else { side },
                    if d.y < 0. { -side } else { side },
                );
            } else {
                let angle =
                    (d.angle() / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
                pos = a + Vec2::angled(angle) * d.length();
            }
        }
        if stroke.tool == Tool::Pen {
            if stroke.end().distance(pos) >= 0.7 {
                stroke.points.push(pos);
            }
        } else {
            stroke.points.truncate(1);
            stroke.points.push(pos);
        }
        self.dirty = true;
    }

    fn update_cursor(&mut self) {
        let icon = if self.ui.passthrough
            || self
                .pointer_pos
                .is_some_and(|p| ui::contains_ui(self.size, p, &self.ui))
        {
            CursorIcon::Default
        } else {
            CursorIcon::Crosshair
        };
        if self.cursor != Some(icon)
            && let Some(pointer) = &self.pointer
            && pointer.set_cursor(&self.conn, icon).is_ok()
        {
            self.cursor = Some(icon);
        }
    }

    fn render(&mut self) -> Result<()> {
        self.dirty = false;
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.start.elapsed().as_secs_f64()),
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.events),
            focused: !self.ui.passthrough,
            ..Default::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(self.scale as f32);
            viewport.inner_rect = input.screen_rect;
        }
        let old_panels = (self.ui.help, self.ui.popover);
        let mut actions = Vec::new();
        let output = self.egui.run(input, |ctx| {
            actions = ui::draw(ctx, self.size, &mut self.ui, &self.document);
        });
        if old_panels != (self.ui.help, self.ui.popover) {
            self.sync_input();
        }
        self.ink.clear();
        for stroke in &self.document.strokes {
            stroke.primitives(&mut self.ink);
        }
        if let Some(stroke) = &self.active {
            stroke.primitives(&mut self.ink);
        }
        let jobs = self.egui.tessellate(output.shapes, output.pixels_per_point);
        // wgpu's Wayland backend performs the actual buffer commit in present().
        self.surface()
            .frame(&self.qh, FrameCallbackData(self.surface().clone()));
        self.frame_pending = true;
        self.renderer.as_mut().unwrap().render(
            &self.ink,
            &jobs,
            &output.textures_delta,
            output.pixels_per_point,
        )?;
        if let Some(viewport) = output.viewport_output.get(&egui::ViewportId::ROOT)
            && viewport.repaint_delay < Duration::from_secs(60)
        {
            self.next_repaint = Some(Instant::now() + viewport.repaint_delay);
        }
        // Consume texture deltas before hiding/unmapping; otherwise a font atlas
        // update from this UI frame could be lost when the overlay is shown again.
        for action in actions {
            self.command(match action {
                Action::Undo => Command::Undo,
                Action::Redo => Command::Redo,
                Action::Clear => Command::Clear,
                Action::ToggleInput => Command::Passthrough,
                Action::Hide => Command::Hide,
                Action::Quit => Command::Quit,
            });
        }
        Ok(())
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if self
            .layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            self.scale = factor.max(1);
            surface.set_buffer_scale(self.scale);
            if self.configured {
                self.renderer
                    .as_mut()
                    .unwrap()
                    .resize(self.size, self.scale as f32);
            }
            self.dirty = true;
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            self.size = vec2(configure.new_size.0 as f32, configure.new_size.1 as f32);
        }
        self.configured = true;
        self.renderer
            .as_mut()
            .unwrap()
            .resize(self.size, self.scale as f32);
        self.sync_input();
        self.dirty = true;
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
            }
            Capability::Pointer if self.pointer.is_none() => {
                self.pointer = self
                    .seat_state
                    .get_pointer_with_theme::<Self, ()>(
                        qh,
                        &seat,
                        self.shm.wl_shm(),
                        self.compositor.create_surface(qh),
                        ThemeSpec::default(),
                    )
                    .ok();
            }
            _ => {}
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
            }
            Capability::Pointer => {
                self.pointer.take();
                self.finish_gesture();
            }
            _ => {}
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.modifiers = egui::Modifiers::default();
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        // Linux evdev codes keep shortcuts usable with Russian and other layouts.
        let code = event.raw_code;
        if self.modifiers.ctrl {
            match code {
                44 => self.command(if self.modifiers.shift {
                    Command::Redo
                } else {
                    Command::Undo
                }),
                21 => self.command(Command::Redo),
                14 => self.command(Command::Clear),
                16 => self.command(Command::Quit),
                _ => {}
            }
        } else {
            match code {
                1 => {
                    if self.ui.help {
                        self.ui.help = false;
                        self.sync_input();
                    } else {
                        self.command(Command::Hide);
                    }
                }
                15 => self.command(Command::Passthrough),
                25 => self.ui.tool = Tool::Pen,
                38 => self.ui.tool = Tool::Line,
                30 => self.ui.tool = Tool::Arrow,
                19 => self.ui.tool = Tool::Rectangle,
                24 => self.ui.tool = Tool::Ellipse,
                18 => self.ui.tool = Tool::Eraser,
                2..=7 => self.ui.color = (code - 2) as usize,
                26 => self.ui.width_index = self.ui.width_index.saturating_sub(1),
                27 => self.ui.width_index = (self.ui.width_index + 1).min(2),
                59 => {
                    self.ui.popover = None;
                    self.ui.help = !self.ui.help;
                    self.sync_input();
                }
                _ => {}
            }
        }
        self.dirty = true;
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.modifiers = egui::Modifiers {
            alt: modifiers.alt,
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            mac_cmd: false,
            command: modifiers.ctrl,
        };
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if self
                .layer
                .as_ref()
                .is_none_or(|l| l.wl_surface() != &event.surface)
            {
                continue;
            }
            let pos = pos2(event.position.0 as f32, event.position.1 as f32);
            let over_ui = ui::contains_ui(self.size, pos, &self.ui);
            let was_over_ui = self
                .pointer_pos
                .is_some_and(|p| ui::contains_ui(self.size, p, &self.ui));
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.pointer_pos = Some(pos);
                    if over_ui || was_over_ui {
                        self.events.push(Event::PointerMoved(pos));
                        self.dirty = true;
                    }
                    self.draw_to(pos);
                    if matches!(event.kind, PointerEventKind::Enter { .. }) {
                        self.cursor = None;
                    }
                    self.update_cursor();
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_pos = None;
                    self.events.push(Event::PointerGone);
                    self.finish_gesture();
                    self.dirty = true;
                }
                PointerEventKind::Press { button, .. } => {
                    self.pointer_pos = Some(pos);
                    self.events.push(Event::PointerMoved(pos));
                    if button == BTN_LEFT {
                        self.events.push(Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: self.modifiers,
                        });
                        if !over_ui && !self.ui.passthrough {
                            if self.ui.tool == Tool::Eraser {
                                self.erasing = true;
                                self.erase_at(pos);
                            } else {
                                self.active = Some(Stroke {
                                    tool: self.ui.tool,
                                    points: vec![pos],
                                    color: COLORS[self.ui.color],
                                    width: self.ui.width(),
                                });
                            }
                        }
                    } else if button == BTN_RIGHT && !over_ui && !self.ui.passthrough {
                        self.erasing = true;
                        self.erase_at(pos);
                    }
                    self.dirty = true;
                }
                PointerEventKind::Release { button, .. } => {
                    if button == BTN_LEFT {
                        self.events.push(Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                    if button == BTN_LEFT || button == BTN_RIGHT {
                        self.draw_to(pos);
                        self.finish_gesture();
                    }
                    self.dirty = true;
                }
                PointerEventKind::Axis { vertical, .. } => {
                    if !self.ui.passthrough && vertical.absolute.abs() > 0. {
                        self.ui.width_index = if vertical.absolute < 0. {
                            (self.ui.width_index + 1).min(2)
                        } else {
                            self.ui.width_index.saturating_sub(1)
                        };
                        self.dirty = true;
                    }
                }
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}
delegate_registry!(App);
impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}
smithay_client_toolkit::delegate_dispatch2!(App);
