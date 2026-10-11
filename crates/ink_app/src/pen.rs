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
//!   connection reads the pressure from XInput2 on a background thread, and it is handed
//!   to egui as touch events while the pen is the device in use.

// Most of this only runs on Linux.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use eframe::egui;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Everything a pen reported between two `frame` events of the tablet protocol.
#[derive(Clone, Debug, Default)]
struct PenFrame {
    /// The pen came near the window (`Some(true)` for the eraser end) or left it.
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

/// The pressure XInput2 last reported, and whether the pen was the last device to move.
#[derive(Clone, Copy, Debug, Default)]
struct X11Pressure {
    value: f32,
    pen: bool,
}

pub struct PenInput {
    frames: Option<Receiver<PenFrame>>,
    position: Option<egui::Pos2>,
    pressure: f32,
    touching: bool,
    eraser: bool,
    x11: Option<Arc<Mutex<X11Pressure>>>,
    /// The X11 pen is reported to egui as a touch.
    x11_touching: bool,
}

/// Linux input codes of the pen's side buttons, mapped like GTK does.
const BTN_STYLUS: u32 = 0x14b;
const BTN_STYLUS2: u32 = 0x14c;
const BTN_STYLUS3: u32 = 0x149;

impl PenInput {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let x11 = Arc::new(Mutex::new(X11Pressure::default()));
        let backend = start(cc, tx, x11.clone());
        Self {
            frames: (backend == Some(Backend::Wayland)).then_some(rx),
            position: None,
            pressure: 1.0,
            touching: false,
            eraser: false,
            x11: (backend == Some(Backend::X11)).then_some(x11),
            x11_touching: false,
        }
    }

    /// Adds what the pen did since the last frame to egui's input. Call it from eframe's
    /// `raw_input_hook`.
    pub fn feed(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) -> Vec<PenNotice> {
        let mut notices = Vec::new();
        if let Some(x11) = &self.x11 {
            let pressure = *x11.lock().unwrap_or_else(|e| e.into_inner());
            let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
            let phase = match (pressure.pen, self.x11_touching) {
                (true, false) => Some(egui::TouchPhase::Start),
                (true, true) => Some(egui::TouchPhase::Move),
                (false, true) => Some(egui::TouchPhase::End),
                (false, false) => None,
            };
            if let Some(phase) = phase {
                self.x11_touching = pressure.pen;
                // Ahead of this frame's pointer events, so they paint with this pressure.
                raw.events.insert(
                    0,
                    egui::Event::Touch {
                        device_id: egui::TouchDeviceId(u64::from_le_bytes(*b"inkpen\0\0")),
                        id: egui::TouchId(0),
                        phase,
                        pos,
                        force: pressure.pen.then_some(pressure.value),
                    },
                );
            }
        }
        let Some(frames) = &self.frames else {
            return notices;
        };
        let frames: Vec<PenFrame> = frames.try_iter().collect();
        if frames.is_empty() {
            return notices;
        }
        let zoom = ctx.zoom_factor();
        let modifiers = ctx.input(|i| i.modifiers);
        let touch = |phase, pos, force| egui::Event::Touch {
            device_id: egui::TouchDeviceId(u64::from_le_bytes(*b"inkpen\0\0")),
            id: egui::TouchId(0),
            phase,
            pos,
            force,
        };
        for frame in frames {
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
                        .push(touch(egui::TouchPhase::Move, pos, Some(self.pressure)));
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
                        .push(touch(egui::TouchPhase::Start, pos, Some(self.pressure)));
                    raw.events.push(button(egui::PointerButton::Primary, true));
                }
                for (code, pressed) in frame.buttons {
                    let which = match code {
                        BTN_STYLUS => egui::PointerButton::Middle,
                        BTN_STYLUS2 | BTN_STYLUS3 => egui::PointerButton::Secondary,
                        _ => continue,
                    };
                    raw.events.push(button(which, pressed));
                }
                if (frame.contact == Some(false) || leaving) && self.touching {
                    self.touching = false;
                    raw.events.push(button(egui::PointerButton::Primary, false));
                    raw.events.push(touch(egui::TouchPhase::End, pos, None));
                }
            }
            if leaving {
                self.position = None;
                self.touching = false;
                raw.events.push(egui::Event::PointerGone);
                if self.eraser {
                    self.eraser = false;
                    notices.push(PenNotice::EraserOut);
                }
            }
        }
        notices
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
    x11_pressure: Arc<Mutex<X11Pressure>>,
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
        (_, RawWindowHandle::Xlib(handle)) => spawn_x11(handle.window as u32, x11_pressure),
        (_, RawWindowHandle::Xcb(handle)) => spawn_x11(handle.window.get(), x11_pressure),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn spawn_x11(window: u32, pressure: Arc<Mutex<X11Pressure>>) -> Option<Backend> {
    std::thread::Builder::new()
        .name("pen".into())
        .spawn(move || {
            if let Err(err) = x11::run(window, pressure) {
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
    _x11_pressure: Arc<Mutex<X11Pressure>>,
) -> Option<Backend> {
    None
}

/// Pen pressure on X11 through XInput2.
#[cfg(target_os = "linux")]
mod x11 {
    use super::X11Pressure;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xinput::{
        self, ConnectionExt as _, DeviceClassData, EventMask, Fp3232, XIEventMask,
    };
    use x11rb::protocol::xproto::ConnectionExt as _;

    /// XInput2's id for "every device".
    const ALL_DEVICES: xinput::DeviceId = 0;

    /// The pressure axis of a device.
    struct Axis {
        number: u16,
        min: f64,
        max: f64,
    }

    /// Reads pressure for our window until the connection closes.
    pub fn run(window: u32, shared: Arc<Mutex<X11Pressure>>) -> Result<(), String> {
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
            let mut pressure = shared.lock().unwrap_or_else(|e| e.into_inner());
            match axes.get(&device) {
                Some(axis) => {
                    if let Some(value) = valuator(&motion, axis.number) {
                        let range = (axis.max - axis.min).max(1e-9);
                        pressure.value = ((value - axis.min) / range).clamp(0.0, 1.0) as f32;
                    }
                    pressure.pen = true;
                }
                None => pressure.pen = false,
            }
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
}

#[cfg(target_os = "linux")]
mod wayland {
    use super::{PenFrame, Proximity};
    use std::collections::HashMap;
    use std::sync::mpsc::Sender;
    use wayland_client::backend::ObjectId;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_registry::WlRegistry, wl_seat::WlSeat, wl_surface::WlSurface,
    };
    use wayland_client::{
        Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
    };
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
        zwp_tablet_v2::ZwpTabletV2,
    };

    struct State {
        tx: Sender<PenFrame>,
        ctx: eframe::egui::Context,
        /// Protocol id of our window's surface; the pen over other surfaces is ignored.
        window: u32,
        cursor_shapes: Option<WpCursorShapeManagerV1>,
        tools: HashMap<ObjectId, Tool>,
        frame: PenFrame,
        /// The pen is over our window.
        inside: bool,
    }

    #[derive(Default)]
    struct Tool {
        eraser: bool,
        cursor: Option<WpCursorShapeDeviceV1>,
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
        let mut state = State {
            tx,
            ctx,
            window,
            cursor_shapes: globals.bind(&qh, 1..=1, ()).ok(),
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
    delegate_noop!(State: ignore ZwpTabletManagerV2);
    delegate_noop!(State: ignore ZwpTabletV2);
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
                        eraser: false,
                        cursor,
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

    impl Dispatch<ZwpTabletPadV2, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZwpTabletPadV2,
            _: zwp_tablet_pad_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
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
            _: &QueueHandle<Self>,
        ) {
            use zwp_tablet_tool_v2::{ButtonState, Event, Type};
            let frame = &mut state.frame;
            match event {
                Event::Type { tool_type } => {
                    state.tools.entry(tool.id()).or_default().eraser =
                        matches!(tool_type, WEnum::Value(Type::Eraser));
                }
                Event::ProximityIn {
                    serial, surface, ..
                } => {
                    state.inside = surface.id().protocol_id() == state.window;
                    let entry = state.tools.entry(tool.id()).or_default();
                    if state.inside {
                        frame.proximity = Some(Proximity::In {
                            eraser: entry.eraser,
                        });
                        if let Some(cursor) = &entry.cursor {
                            cursor.set_shape(serial, Shape::Default);
                        }
                    }
                }
                Event::ProximityOut => {
                    if state.inside {
                        frame.proximity = Some(Proximity::Out);
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
                    if let Some(cursor) = state.tools.remove(&tool.id()).and_then(|t| t.cursor) {
                        cursor.destroy();
                    }
                    tool.destroy();
                }
                _ if !state.inside => {}
                Event::Motion { x, y } => frame.position = Some([x as f32, y as f32]),
                Event::Pressure { pressure } => frame.pressure = Some(pressure as f32 / 65535.0),
                Event::Down { .. } => frame.contact = Some(true),
                Event::Up => frame.contact = Some(false),
                Event::Button {
                    button, state: s, ..
                } => frame
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

    fn pen(frames: Vec<PenFrame>) -> (Vec<egui::Event>, Vec<PenNotice>) {
        let (tx, rx) = channel();
        for frame in frames {
            tx.send(frame).unwrap();
        }
        let mut input = PenInput {
            frames: Some(rx),
            position: None,
            pressure: 1.0,
            touching: false,
            eraser: false,
            x11: None,
            x11_touching: false,
        };
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        let notices = input.feed(&ctx, &mut raw);
        (raw.events, notices)
    }

    #[test]
    fn a_pen_stroke_becomes_pointer_and_touch_events() {
        let (events, notices) = pen(vec![
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
        let kinds: Vec<String> = events
            .iter()
            .map(|e| match e {
                egui::Event::PointerMoved(p) => format!("move {} {}", p.x, p.y),
                egui::Event::PointerButton { pressed, .. } => format!("button {pressed}"),
                egui::Event::Touch { phase, force, .. } => format!("touch {phase:?} {force:?}"),
                egui::Event::PointerGone => "gone".into(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "move 10 20",
                "touch Start Some(0.25)",
                "button true",
                "touch Move Some(0.5)",
                "move 12 20",
                "button false",
                "touch End None",
                "gone",
            ]
        );
    }

    #[test]
    fn the_eraser_end_is_reported() {
        let (_, notices) = pen(vec![
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
}
