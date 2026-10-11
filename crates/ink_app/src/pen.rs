//! Pen tablets.
//!
//! - **Windows:** winit reports pens (Windows Ink, like Painter) as touches with a force,
//!   and egui turns them into pointer events. Nothing to do here.
//! - **Wayland:** GNOME and KDE send pen input only through the tablet protocol
//!   (`zwp_tablet_v2`), which winit 0.30 does not speak, so without this module a pen does
//!   nothing at all. A small client on a background thread shares winit's connection,
//!   listens to the pen and hands it to egui as pointer events, plus touch events whose
//!   force carries the pressure (as on Windows).
//! - **X11:** the pen moves the mouse pointer, so winit sees it as a mouse. A second
//!   connection reads the pressure from XInput2 on a background thread, and each pointer
//!   event gets a touch event with the pressure XInput2 reported at that position.

// Most of this only runs on Linux.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use eframe::egui;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Everything a pen reported between two `frame` events of the tablet protocol.
#[derive(Clone, Debug, Default)]
struct PenFrame {
    /// The pen came near the window or left it.
    proximity: Option<Proximity>,
    /// Position in logical pixels from the window's top left.
    position: Option<[f32; 2]>,
    pressure: Option<f32>,
    /// The tip touched (true) or left (false) the tablet.
    contact: Option<bool>,
    /// Side buttons as Linux input codes.
    buttons: Vec<(u32, bool)>,
}

#[derive(Clone, Copy, Debug)]
enum Proximity {
    In { eraser: bool },
    Out,
}

/// What the pen did that the app reacts to beyond pointer input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenNotice {
    /// The eraser end came near the window.
    EraserIn,
    /// The eraser end left.
    EraserOut,
}

/// One motion XInput2 reported over the window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct X11Sample {
    /// Physical pixels from the window's top left.
    position: [f32; 2],
    pressure: f32,
    /// The device has a pressure axis.
    pen: bool,
}

/// Samples from the X11 thread, oldest first.
type X11Samples = Arc<Mutex<VecDeque<X11Sample>>>;

pub struct PenInput {
    frames: Option<Receiver<PenFrame>>,
    position: Option<egui::Pos2>,
    pressure: f32,
    touching: bool,
    eraser: bool,
    /// Side buttons held down, released when the pen leaves.
    side_buttons: Vec<egui::PointerButton>,
    x11: Option<X11Samples>,
    /// The newest X11 sample handed out.
    x11_last: Option<X11Sample>,
    /// The X11 pen is reported to egui as a touch.
    x11_touching: bool,
}

/// Linux input codes of the pen's side buttons, mapped like GTK does.
const BTN_STYLUS: u32 = 0x14b;
const BTN_STYLUS2: u32 = 0x14c;
const BTN_STYLUS3: u32 = 0x149;

/// The id of the touch that stands for the pen.
fn pen_touch(phase: egui::TouchPhase, pos: egui::Pos2, force: Option<f32>) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(u64::from_le_bytes(*b"inkpen\0\0")),
        id: egui::TouchId(0),
        phase,
        pos,
        force,
    }
}

impl PenInput {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let samples = X11Samples::default();
        let backend = start(cc, tx, samples.clone());
        Self {
            frames: (backend == Some(Backend::Wayland)).then_some(rx),
            x11: (backend == Some(Backend::X11)).then_some(samples),
            ..Self::idle()
        }
    }

    fn idle() -> Self {
        Self {
            frames: None,
            position: None,
            pressure: 1.0,
            touching: false,
            eraser: false,
            side_buttons: Vec::new(),
            x11: None,
            x11_last: None,
            x11_touching: false,
        }
    }

    /// Adds what the pen did since the last frame to egui's input. Call it from eframe's
    /// `raw_input_hook`.
    pub fn feed(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) -> Vec<PenNotice> {
        if let Some(samples) = &self.x11 {
            let samples: Vec<X11Sample> = samples
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .collect();
            self.feed_x11(ctx.pixels_per_point(), raw, samples);
        }
        let mut notices = Vec::new();
        if let Some(frames) = &self.frames {
            let frames: Vec<PenFrame> = frames.try_iter().collect();
            let modifiers = ctx.input(|i| i.modifiers);
            for frame in frames {
                self.feed_frame(ctx.zoom_factor(), modifiers, frame, raw, &mut notices);
            }
        }
        notices
    }

    /// Puts a touch with the pen's pressure in front of every pointer event that XInput2
    /// saw the pen make. Adds nothing when the frame has no pointer events, so an idle pen
    /// doesn't keep the app redrawing.
    fn feed_x11(
        &mut self,
        pixels_per_point: f32,
        raw: &mut egui::RawInput,
        samples: Vec<X11Sample>,
    ) {
        let mut samples: VecDeque<X11Sample> = samples.into();
        let mut events = Vec::with_capacity(raw.events.len() * 2);
        for event in raw.events.drain(..) {
            let pos = match &event {
                egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } => {
                    Some(*pos)
                }
                _ => None,
            };
            if let Some(pos) = pos {
                // The sample reported at this position; earlier ones are past.
                let at = |s: &X11Sample| {
                    (s.position[0] / pixels_per_point - pos.x).abs() < 1.5
                        && (s.position[1] / pixels_per_point - pos.y).abs() < 1.5
                };
                if let Some(k) = samples.iter().position(at) {
                    self.x11_last = samples.drain(..=k).next_back();
                }
                if let Some(sample) = self.x11_last {
                    let phase = match (sample.pen, self.x11_touching) {
                        (true, false) => Some(egui::TouchPhase::Start),
                        (true, true) => Some(egui::TouchPhase::Move),
                        (false, true) => Some(egui::TouchPhase::End),
                        (false, false) => None,
                    };
                    if let Some(phase) = phase {
                        self.x11_touching = sample.pen;
                        events.push(pen_touch(phase, pos, sample.pen.then_some(sample.pressure)));
                    }
                }
            }
            events.push(event);
        }
        raw.events = events;
        if let Some(newest) = samples.back() {
            self.x11_last = Some(*newest);
        }
    }

    fn feed_frame(
        &mut self,
        zoom: f32,
        modifiers: egui::Modifiers,
        frame: PenFrame,
        raw: &mut egui::RawInput,
        notices: &mut Vec<PenNotice>,
    ) {
        if let Some(Proximity::In { eraser }) = frame.proximity
            && eraser != self.eraser
        {
            self.eraser = eraser;
            notices.push(if eraser {
                PenNotice::EraserIn
            } else {
                PenNotice::EraserOut
            });
        }
        if let Some(pressure) = frame.pressure {
            self.pressure = pressure.clamp(0.0, 1.0);
        }
        if let Some([x, y]) = frame.position {
            let pos = egui::pos2(x / zoom, y / zoom);
            self.position = Some(pos);
            if self.touching {
                raw.events
                    .push(pen_touch(egui::TouchPhase::Move, pos, Some(self.pressure)));
            }
            raw.events.push(egui::Event::PointerMoved(pos));
        }
        let leaving = matches!(frame.proximity, Some(Proximity::Out));
        if let Some(pos) = self.position {
            let button = |button, pressed| egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            };
            if frame.contact == Some(true) && !self.touching {
                self.touching = true;
                raw.events
                    .push(pen_touch(egui::TouchPhase::Start, pos, Some(self.pressure)));
                raw.events.push(button(egui::PointerButton::Primary, true));
            }
            for (code, pressed) in frame.buttons {
                let which = match code {
                    BTN_STYLUS => egui::PointerButton::Middle,
                    BTN_STYLUS2 | BTN_STYLUS3 => egui::PointerButton::Secondary,
                    _ => continue,
                };
                self.side_buttons.retain(|&b| b != which);
                if pressed {
                    self.side_buttons.push(which);
                }
                raw.events.push(button(which, pressed));
            }
            if (frame.contact == Some(false) || leaving) && self.touching {
                self.touching = false;
                raw.events.push(button(egui::PointerButton::Primary, false));
                raw.events.push(pen_touch(egui::TouchPhase::End, pos, None));
            }
            if leaving {
                // The release may go to another window once the pen has left this one.
                for held in self.side_buttons.drain(..) {
                    raw.events.push(button(held, false));
                }
            }
        }
        if leaving {
            self.position = None;
            self.touching = false;
            self.side_buttons.clear();
            raw.events.push(egui::Event::PointerGone);
            if self.eraser {
                self.eraser = false;
                notices.push(PenNotice::EraserOut);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Wayland,
    X11,
}

/// Starts listening to pens on Linux. Returns which way, if any.
#[cfg(target_os = "linux")]
fn start(
    cc: &eframe::CreationContext<'_>,
    tx: Sender<PenFrame>,
    x11_samples: X11Samples,
) -> Option<Backend> {
    use raw_window_handle::{
        HasDisplayHandle as _, HasWindowHandle as _, RawDisplayHandle, RawWindowHandle,
    };
    let (Some(window), Ok(display), Ok(handle)) =
        (cc.winit_window(), cc.display_handle(), cc.window_handle())
    else {
        return None;
    };
    match (display.as_raw(), handle.as_raw()) {
        (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) => {
            // Raw pointers are not Send; the window kept alive below keeps them valid.
            let display = display.display.as_ptr() as usize;
            let surface = surface.surface.as_ptr() as usize;
            let window = window.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::Builder::new()
                .name("pen".into())
                .spawn(move || {
                    let _keep_alive = window;
                    // SAFETY: the window holds winit's Wayland connection and surface, and
                    // the thread holds the window.
                    let result =
                        unsafe { wayland::run(display as *mut _, surface as *mut _, tx, ctx) };
                    if let Err(err) = result {
                        log::warn!("No pen tablet support: {err}");
                    }
                })
                .ok()
                .map(|_| Backend::Wayland)
        }
        (_, RawWindowHandle::Xlib(handle)) => spawn_x11(handle.window as u32, x11_samples),
        (_, RawWindowHandle::Xcb(handle)) => spawn_x11(handle.window.get(), x11_samples),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn spawn_x11(window: u32, samples: X11Samples) -> Option<Backend> {
    std::thread::Builder::new()
        .name("pen".into())
        .spawn(move || {
            if let Err(err) = x11::run(window, samples) {
                log::warn!("No pen pressure: {err}");
            }
        })
        .ok()
        .map(|_| Backend::X11)
}

#[cfg(not(target_os = "linux"))]
fn start(
    _cc: &eframe::CreationContext<'_>,
    _tx: Sender<PenFrame>,
    _x11_samples: X11Samples,
) -> Option<Backend> {
    None
}

/// Pen pressure on X11 through XInput2.
#[cfg(target_os = "linux")]
mod x11 {
    use super::{X11Sample, X11Samples};
    use std::collections::{HashMap, HashSet};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xinput::{
        self, ConnectionExt as _, DeviceClassData, EventMask, Fp3232, XIEventMask,
    };
    use x11rb::protocol::xproto::ConnectionExt as _;

    /// XInput2's id for "every device".
    const ALL_DEVICES: xinput::DeviceId = 0;
    /// Samples kept for the UI thread; it takes them every frame.
    const MAX_SAMPLES: usize = 1024;

    /// The pressure axis of a device.
    struct Axis {
        number: u16,
        min: f64,
        max: f64,
    }

    /// Reports motions over our window until the connection closes.
    pub fn run(window: u32, samples: X11Samples) -> Result<(), String> {
        let error = |e: &dyn std::fmt::Display| e.to_string();
        let (conn, _) = x11rb::connect(None).map_err(|e| error(&e))?;
        conn.xinput_xi_query_version(2, 0)
            .map_err(|e| error(&e))?
            .reply()
            .map_err(|_| "the X server has no XInput2".to_owned())?;
        let label = conn
            .intern_atom(false, b"Abs Pressure")
            .map_err(|e| error(&e))?
            .reply()
            .map_err(|e| error(&e))?
            .atom;
        let mut axes = pressure_axes(&conn, label)?;
        let mut others: HashSet<xinput::DeviceId> = HashSet::new();
        let mut pressures: HashMap<xinput::DeviceId, f32> = HashMap::new();
        conn.xinput_xi_select_events(
            window,
            &[EventMask {
                deviceid: ALL_DEVICES,
                mask: vec![XIEventMask::MOTION],
            }],
        )
        .map_err(|e| error(&e))?;
        conn.flush().map_err(|e| error(&e))?;
        loop {
            let event = conn.wait_for_event().map_err(|e| error(&e))?;
            let Event::XinputMotion(motion) = event else {
                continue;
            };
            // Each motion arrives from the physical device and again from the master
            // pointer; the physical one carries that device's own axes.
            if motion.deviceid != motion.sourceid {
                continue;
            }
            let device = motion.sourceid;
            if !axes.contains_key(&device) && !others.contains(&device) {
                // A device we haven't seen: plugged in after startup, or not a pen.
                axes = pressure_axes(&conn, label)?;
                if !axes.contains_key(&device) {
                    others.insert(device);
                }
            }
            let sample = match axes.get(&device) {
                Some(axis) => {
                    let pressure = pressures.entry(device).or_insert(0.0);
                    if let Some(value) = valuator(&motion, axis.number) {
                        let range = (axis.max - axis.min).max(1e-9);
                        *pressure = ((value - axis.min) / range).clamp(0.0, 1.0) as f32;
                    }
                    X11Sample {
                        position: [fixed16(motion.event_x), fixed16(motion.event_y)],
                        pressure: *pressure,
                        pen: true,
                    }
                }
                None => X11Sample {
                    position: [fixed16(motion.event_x), fixed16(motion.event_y)],
                    pressure: 1.0,
                    pen: false,
                },
            };
            let mut samples = samples.lock().unwrap_or_else(|e| e.into_inner());
            if samples.len() >= MAX_SAMPLES {
                samples.pop_front();
            }
            samples.push_back(sample);
        }
    }

    /// The devices that report pressure, by id.
    fn pressure_axes(
        conn: &impl Connection,
        label: u32,
    ) -> Result<HashMap<xinput::DeviceId, Axis>, String> {
        let devices = conn
            .xinput_xi_query_device(ALL_DEVICES)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let mut axes = HashMap::new();
        for device in devices.infos {
            for class in &device.classes {
                if let DeviceClassData::Valuator(v) = &class.data
                    && v.label == label
                {
                    axes.insert(
                        device.deviceid,
                        Axis {
                            number: v.number,
                            min: fixed(&v.min),
                            max: fixed(&v.max),
                        },
                    );
                }
            }
        }
        Ok(axes)
    }

    /// The value of valuator `number` if the event carries it. `axisvalues` holds only the
    /// valuators whose bit is set in `valuator_mask`, in order.
    fn valuator(event: &xinput::MotionEvent, number: u16) -> Option<f64> {
        let set = |bit: u16| {
            event
                .valuator_mask
                .get(bit as usize / 32)
                .is_some_and(|word| word & (1 << (bit % 32)) != 0)
        };
        if !set(number) {
            return None;
        }
        let index = (0..number).filter(|&bit| set(bit)).count();
        event.axisvalues.get(index).map(fixed)
    }

    fn fixed(v: &Fp3232) -> f64 {
        v.integral as f64 + v.frac as f64 / 4_294_967_296.0
    }

    fn fixed16(v: xinput::Fp1616) -> f32 {
        v as f32 / 65536.0
    }
}

#[cfg(target_os = "linux")]
mod wayland {
    use super::{PenFrame, Proximity};
    use std::collections::HashMap;
    use std::sync::mpsc::Sender;
    use wayland_client::backend::ObjectId;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_seat::WlSeat, wl_shm::WlShm,
        wl_surface::WlSurface,
    };
    use wayland_client::{
        Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
    };
    use wayland_cursor::CursorTheme;
    use wayland_protocols::wp::cursor_shape::v1::client::{
        wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
    };
    use wayland_protocols::wp::tablet::zv2::client::{
        zwp_tablet_manager_v2::ZwpTabletManagerV2,
        zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
        zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
        zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
        zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
        zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
        zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
        zwp_tablet_v2::{self, ZwpTabletV2},
    };

    /// Size of the pen's cursor when it comes from the cursor theme.
    const CURSOR_SIZE: u32 = 24;

    struct State {
        conn: Connection,
        tx: Sender<PenFrame>,
        ctx: eframe::egui::Context,
        /// Protocol id of our window's surface; the pen over other surfaces is ignored.
        window: u32,
        cursor_shapes: Option<WpCursorShapeManagerV1>,
        /// Without cursor shapes, the cursor comes from the theme.
        compositor: Option<WlCompositor>,
        shm: Option<WlShm>,
        theme: Option<CursorTheme>,
        tools: HashMap<ObjectId, Tool>,
        frame: PenFrame,
        /// The pen is over our window.
        inside: bool,
    }

    #[derive(Default)]
    struct Tool {
        eraser: bool,
        cursor: Option<WpCursorShapeDeviceV1>,
        cursor_surface: Option<WlSurface>,
    }

    /// Listens to pens until the connection closes. Blocks the calling thread.
    ///
    /// # Safety
    ///
    /// `display` must be a valid `wl_display` and `surface` a `wl_surface` on it, and both
    /// must stay valid while this runs.
    pub unsafe fn run(
        display: *mut std::ffi::c_void,
        surface: *mut std::ffi::c_void,
        tx: Sender<PenFrame>,
        ctx: eframe::egui::Context,
    ) -> Result<(), String> {
        // SAFETY: guaranteed by the caller.
        let backend =
            unsafe { wayland_client::backend::Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        // SAFETY: guaranteed by the caller.
        let window = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }
            .map_err(|e| e.to_string())?
            .protocol_id();
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        // Version 1 is enough; version 2 adds pad dials, which would need handling.
        let manager = globals
            .bind::<ZwpTabletManagerV2, _, _>(&qh, 1..=1, ())
            .map_err(|_| "the compositor does not support the tablet protocol".to_owned())?;
        let seat = globals
            .bind::<WlSeat, _, _>(&qh, 1..=1, ())
            .map_err(|e| e.to_string())?;
        manager.get_tablet_seat(&seat, &qh, ());
        let cursor_shapes = globals.bind(&qh, 1..=1, ()).ok();
        let (compositor, shm) = if cursor_shapes.is_none() {
            (
                globals.bind(&qh, 1..=4, ()).ok(),
                globals.bind(&qh, 1..=1, ()).ok(),
            )
        } else {
            (None, None)
        };
        let mut state = State {
            conn: conn.clone(),
            tx,
            ctx,
            window,
            cursor_shapes,
            compositor,
            shm,
            theme: None,
            tools: HashMap::new(),
            frame: PenFrame::default(),
            inside: false,
        };
        // Reads the shared socket alongside winit; libwayland keeps the queues apart.
        loop {
            queue
                .blocking_dispatch(&mut state)
                .map_err(|e| e.to_string())?;
        }
    }

    impl State {
        /// Shows the default cursor for the tool: by shape where the compositor knows
        /// cursor shapes (GNOME 48, Plasma 6), else as an image from the cursor theme.
        fn set_cursor(&mut self, tool: &ZwpTabletToolV2, serial: u32, qh: &QueueHandle<Self>) {
            let Some(entry) = self.tools.get_mut(&tool.id()) else {
                return;
            };
            if let Some(device) = &entry.cursor {
                device.set_shape(serial, Shape::Default);
                return;
            }
            let (Some(compositor), Some(shm)) = (&self.compositor, &self.shm) else {
                return;
            };
            if self.theme.is_none() {
                self.theme = CursorTheme::load(&self.conn, shm.clone(), CURSOR_SIZE).ok();
            }
            let Some(theme) = &mut self.theme else { return };
            let name = if theme.get_cursor("default").is_some() {
                "default"
            } else {
                "left_ptr"
            };
            let Some(cursor) = theme.get_cursor(name) else {
                return;
            };
            let image = &cursor[0];
            let (hx, hy) = image.hotspot();
            let (w, h) = image.dimensions();
            // One surface per tool, as the protocol asks.
            let surface = entry
                .cursor_surface
                .get_or_insert_with(|| compositor.create_surface(qh, ()));
            surface.attach(Some(&**image), 0, 0);
            surface.damage(0, 0, w as i32, h as i32);
            surface.commit();
            tool.set_cursor(serial, Some(surface), hx as i32, hy as i32);
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    delegate_noop!(State: ignore WlSeat);
    delegate_noop!(State: WlCompositor);
    delegate_noop!(State: ignore WlShm);
    delegate_noop!(State: ignore WlSurface);
    delegate_noop!(State: ignore ZwpTabletManagerV2);
    delegate_noop!(State: ignore ZwpTabletPadRingV2);
    delegate_noop!(State: ignore ZwpTabletPadStripV2);
    delegate_noop!(State: ignore WpCursorShapeManagerV1);
    delegate_noop!(State: ignore WpCursorShapeDeviceV1);

    impl Dispatch<ZwpTabletSeatV2, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZwpTabletSeatV2,
            event: zwp_tablet_seat_v2::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let zwp_tablet_seat_v2::Event::ToolAdded { id } = event {
                let cursor = state
                    .cursor_shapes
                    .as_ref()
                    .map(|m| m.get_tablet_tool_v2(&id, qh, ()));
                state.tools.insert(
                    id.id(),
                    Tool {
                        cursor,
                        ..Default::default()
                    },
                );
            }
        }

        event_created_child!(State, ZwpTabletSeatV2, [
            zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
            zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
            zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
        ]);
    }

    impl Dispatch<ZwpTabletV2, ()> for State {
        fn event(
            _: &mut Self,
            tablet: &ZwpTabletV2,
            event: zwp_tablet_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zwp_tablet_v2::Event::Removed = event {
                tablet.destroy();
            }
        }
    }

    impl Dispatch<ZwpTabletPadV2, ()> for State {
        fn event(
            _: &mut Self,
            pad: &ZwpTabletPadV2,
            event: zwp_tablet_pad_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zwp_tablet_pad_v2::Event::Removed = event {
                pad.destroy();
            }
        }

        event_created_child!(State, ZwpTabletPadV2, [
            zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
        ]);
    }

    impl Dispatch<ZwpTabletPadGroupV2, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZwpTabletPadGroupV2,
            _: zwp_tablet_pad_group_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }

        event_created_child!(State, ZwpTabletPadGroupV2, [
            zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
            zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
        ]);
    }

    impl Dispatch<ZwpTabletToolV2, ()> for State {
        fn event(
            state: &mut Self,
            tool: &ZwpTabletToolV2,
            event: zwp_tablet_tool_v2::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            use zwp_tablet_tool_v2::{ButtonState, Event, Type};
            match event {
                Event::Type { tool_type } => {
                    state.tools.entry(tool.id()).or_default().eraser =
                        matches!(tool_type, WEnum::Value(Type::Eraser));
                }
                Event::ProximityIn {
                    serial, surface, ..
                } => {
                    state.inside = surface.id().protocol_id() == state.window;
                    if state.inside {
                        let eraser = state.tools.entry(tool.id()).or_default().eraser;
                        state.frame.proximity = Some(Proximity::In { eraser });
                        state.set_cursor(tool, serial, qh);
                    }
                }
                Event::ProximityOut => {
                    if state.inside {
                        state.frame.proximity = Some(Proximity::Out);
                    }
                    state.inside = false;
                }
                Event::Frame { .. } => {
                    let frame = std::mem::take(&mut state.frame);
                    let empty = frame.proximity.is_none()
                        && frame.position.is_none()
                        && frame.pressure.is_none()
                        && frame.contact.is_none()
                        && frame.buttons.is_empty();
                    if !empty && state.tx.send(frame).is_ok() {
                        // Wakes eframe's event loop, which knows nothing about the pen.
                        state.ctx.request_repaint();
                    }
                }
                Event::Removed => {
                    if let Some(entry) = state.tools.remove(&tool.id()) {
                        if let Some(cursor) = entry.cursor {
                            cursor.destroy();
                        }
                        if let Some(surface) = entry.cursor_surface {
                            surface.destroy();
                        }
                    }
                    tool.destroy();
                }
                _ if !state.inside => {}
                Event::Motion { x, y } => state.frame.position = Some([x as f32, y as f32]),
                Event::Pressure { pressure } => {
                    state.frame.pressure = Some(pressure as f32 / 65535.0)
                }
                Event::Down { .. } => state.frame.contact = Some(true),
                Event::Up => state.frame.contact = Some(false),
                Event::Button {
                    button, state: s, ..
                } => state
                    .frame
                    .buttons
                    .push((button, matches!(s, WEnum::Value(ButtonState::Pressed)))),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(events: &[egui::Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                egui::Event::PointerMoved(p) => format!("move {} {}", p.x, p.y),
                egui::Event::PointerButton {
                    button, pressed, ..
                } => format!("{button:?} {pressed}"),
                egui::Event::Touch { phase, force, .. } => format!("touch {phase:?} {force:?}"),
                egui::Event::PointerGone => "gone".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn wayland(frames: Vec<PenFrame>) -> (Vec<egui::Event>, Vec<PenNotice>) {
        let (tx, rx) = channel();
        for frame in frames {
            tx.send(frame).unwrap();
        }
        let mut input = PenInput {
            frames: Some(rx),
            ..PenInput::idle()
        };
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        let notices = input.feed(&ctx, &mut raw);
        (raw.events, notices)
    }

    #[test]
    fn a_pen_stroke_becomes_pointer_and_touch_events() {
        let (events, notices) = wayland(vec![
            PenFrame {
                proximity: Some(Proximity::In { eraser: false }),
                position: Some([10.0, 20.0]),
                ..Default::default()
            },
            PenFrame {
                contact: Some(true),
                pressure: Some(0.25),
                ..Default::default()
            },
            PenFrame {
                position: Some([12.0, 20.0]),
                pressure: Some(0.5),
                ..Default::default()
            },
            PenFrame {
                contact: Some(false),
                proximity: Some(Proximity::Out),
                ..Default::default()
            },
        ]);
        assert!(notices.is_empty());
        assert_eq!(
            describe(&events),
            [
                "move 10 20",
                "touch Start Some(0.25)",
                "Primary true",
                "touch Move Some(0.5)",
                "move 12 20",
                "Primary false",
                "touch End None",
                "gone",
            ]
        );
    }

    #[test]
    fn side_buttons_are_released_when_the_pen_leaves() {
        let (events, _) = wayland(vec![
            PenFrame {
                proximity: Some(Proximity::In { eraser: false }),
                position: Some([1.0, 1.0]),
                buttons: vec![(BTN_STYLUS, true)],
                ..Default::default()
            },
            PenFrame {
                proximity: Some(Proximity::Out),
                ..Default::default()
            },
        ]);
        assert_eq!(
            describe(&events),
            ["move 1 1", "Middle true", "Middle false", "gone"]
        );
    }

    #[test]
    fn the_eraser_end_is_reported() {
        let (_, notices) = wayland(vec![
            PenFrame {
                proximity: Some(Proximity::In { eraser: true }),
                ..Default::default()
            },
            PenFrame {
                proximity: Some(Proximity::Out),
                ..Default::default()
            },
        ]);
        assert_eq!(notices, [PenNotice::EraserIn, PenNotice::EraserOut]);
    }

    #[test]
    fn x11_pointer_events_get_the_pressure_reported_there() {
        let mut input = PenInput::idle();
        let sample = |x: f32, pressure, pen| X11Sample {
            position: [x * 2.0, 0.0],
            pressure,
            pen,
        };
        let moved = |x: f32| egui::Event::PointerMoved(egui::pos2(x, 0.0));
        let mut raw = egui::RawInput {
            events: vec![moved(1.0), moved(2.0), moved(3.0)],
            ..Default::default()
        };
        // At 2 pixels per point; the third event has no sample of its own.
        input.feed_x11(
            2.0,
            &mut raw,
            vec![sample(1.0, 0.2, true), sample(2.0, 0.4, true)],
        );
        assert_eq!(
            describe(&raw.events),
            [
                "touch Start Some(0.2)",
                "move 1 0",
                "touch Move Some(0.4)",
                "move 2 0",
                "touch Move Some(0.4)",
                "move 3 0",
            ]
        );

        // No pointer events: nothing added, so an idle pen doesn't keep egui redrawing.
        let mut raw = egui::RawInput::default();
        input.feed_x11(2.0, &mut raw, vec![sample(5.0, 0.9, true)]);
        assert!(raw.events.is_empty());

        // The mouse moves: the pen's touch ends.
        let mut raw = egui::RawInput {
            events: vec![moved(7.0)],
            ..Default::default()
        };
        input.feed_x11(2.0, &mut raw, vec![sample(7.0, 1.0, false)]);
        assert_eq!(describe(&raw.events), ["touch End None", "move 7 0"]);
    }
}
