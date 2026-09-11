//! The lock: an ext-session-lock-v1 lock, a lock surface on every output, the keyboard into the
//! field, and PAM on its own thread so the screen keeps drawing while it checks.

use std::fs;
use std::thread;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{self, Sender};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::session_lock::{
    SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
    SessionLockSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_output, delegate_registry, delegate_seat,
    delegate_session_lock, delegate_shm, registry_handlers,
};
use umbra_lock::draw::{self, Canvas, Fonts, View};
use umbra_lock::entry::{Entry, Key, Verdict};
use umbra_lock::owner::{self, Owner};

use crate::pam;

/// Locks the session and returns once the owner's password has unlocked it.
pub fn run() -> Result<(), String> {
    let uid = rustix::process::getuid().as_raw();
    let passwd = fs::read_to_string("/etc/passwd")
        .map_err(|e| format!("Could not read /etc/passwd: {e}"))?;
    let owner =
        owner::find(&passwd, uid).ok_or_else(|| format!("User {uid} is not in /etc/passwd."))?;

    let connection = Connection::connect_to_env()
        .map_err(|e| format!("Could not connect to the Wayland session: {e}"))?;
    let (globals, queue) = registry_queue_init::<LockScreen>(&connection)
        .map_err(|e| format!("Could not read the compositor's globals: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, LockScreen> =
        EventLoop::try_new().map_err(|e| format!("Could not start the event loop: {e}"))?;
    let (verdicts, answers) = channel::channel();
    event_loop
        .handle()
        .insert_source(answers, |event, (), lock| {
            if let channel::Event::Msg(verdict) = event {
                lock.checked(verdict);
            }
        })
        .map_err(|e| format!("Could not wait for PAM: {}", e.error))?;
    WaylandSource::new(connection.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|e| format!("Could not wait for the compositor: {}", e.error))?;

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|e| format!("The compositor has no wl_compositor: {e}"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("The compositor has no wl_shm: {e}"))?;
    let pool = SlotPool::new(4 * 1280 * 800, &shm)
        .map_err(|e| format!("Could not make a buffer pool: {e}"))?;
    let mut lock = LockScreen {
        connection,
        loop_handle: event_loop.handle(),
        registry: RegistryState::new(&globals),
        compositor,
        outputs: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh),
        shm,
        pool,
        sessions: SessionLockState::new(&globals, &qh),
        lock: None,
        locked: false,
        unlock: false,
        screens: Vec::new(),
        keyboard: None,
        ctrl: false,
        owner,
        fonts: Fonts::load(),
        entry: Entry::default(),
        verdicts,
        done: None,
    };
    let session_lock = lock
        .sessions
        .lock(&qh)
        .map_err(|e| format!("The compositor cannot lock the session: {e}"))?;
    lock.lock = Some(session_lock);
    let outputs: Vec<_> = lock.outputs.outputs().collect();
    for output in outputs {
        lock.add_screen(&qh, output);
    }

    while lock.done.is_none() {
        event_loop
            .dispatch(None, &mut lock)
            .map_err(|e| format!("The event loop stopped: {e}"))?;
    }
    lock.done.take().unwrap_or(Ok(()))
}

/// One output's lock surface.
struct Screen {
    output: wl_output::WlOutput,
    surface: SessionLockSurface,
    /// The size the compositor asked for, in logical pixels, once it has.
    size: Option<(u32, u32)>,
    scale: i32,
}

struct LockScreen {
    connection: Connection,
    loop_handle: LoopHandle<'static, LockScreen>,
    registry: RegistryState,
    compositor: CompositorState,
    outputs: OutputState,
    seats: SeatState,
    shm: Shm,
    pool: SlotPool,
    sessions: SessionLockState,
    lock: Option<SessionLock>,
    /// The compositor has confirmed the lock: nothing of the session is on screen.
    locked: bool,
    /// PAM accepted the password.
    unlock: bool,
    screens: Vec<Screen>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    ctrl: bool,
    owner: Owner,
    fonts: Fonts,
    entry: Entry,
    verdicts: Sender<Verdict>,
    done: Option<Result<(), String>>,
}

impl LockScreen {
    fn add_screen(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(lock) = &self.lock else {
            return;
        };
        if self.screens.iter().any(|screen| screen.output == output) {
            return;
        }
        let surface = lock.create_lock_surface(self.compositor.create_surface(qh), &output, qh);
        self.screens.push(Screen {
            output,
            surface,
            size: None,
            scale: 1,
        });
    }

    fn draw_all(&mut self) {
        for index in 0..self.screens.len() {
            self.draw(index);
        }
    }

    fn draw(&mut self, index: usize) {
        let Some(screen) = self.screens.get(index) else {
            return;
        };
        let Some((width, height)) = screen.size else {
            return;
        };
        let scale = screen.scale.max(1);
        let (width, height) = (width * scale.unsigned_abs(), height * scale.unsigned_abs());
        let (Ok(w), Ok(h), Ok(stride)) = (
            i32::try_from(width),
            i32::try_from(height),
            i32::try_from(width * 4),
        ) else {
            return;
        };
        let (buffer, pixels) = match self
            .pool
            .create_buffer(w, h, stride, wl_shm::Format::Argb8888)
        {
            Ok(created) => created,
            Err(e) => {
                eprintln!("umbra-lock: could not make a {width}x{height} buffer: {e}");
                return;
            }
        };
        let view = View {
            name: &self.owner.name,
            typed: self.entry.len(),
            status: self.entry.status(),
        };
        draw::paint(
            &mut Canvas::new(pixels, width, height),
            &self.fonts,
            &view,
            scale.unsigned_abs(),
        );
        // the pool keeps the memory until the compositor releases the buffer
        let surface = screen.surface.wl_surface();
        surface.set_buffer_scale(scale);
        if let Err(e) = buffer.attach_to(surface) {
            eprintln!("umbra-lock: could not attach the buffer: {e:?}");
            return;
        }
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }

    fn press(&mut self, event: &KeyEvent) {
        let key = match event.keysym {
            Keysym::Return | Keysym::KP_Enter => Key::Enter,
            Keysym::BackSpace => Key::Backspace,
            Keysym::Escape => Key::Escape,
            _ => match &event.utf8 {
                Some(text) if !self.ctrl => Key::Text(text),
                _ => return,
            },
        };
        if let Some(password) = self.entry.key(key) {
            let user = self.owner.user.clone();
            let verdicts = self.verdicts.clone();
            let started = thread::Builder::new()
                .name("pam".to_string())
                .spawn(move || {
                    // the loop only goes away when the lock screen is ending anyway
                    let _ = verdicts.send(pam::check(&user, password));
                });
            if let Err(e) = started {
                eprintln!("umbra-lock: could not start checking the password: {e}");
                self.entry.checked(Verdict::Failed);
            }
        }
        self.draw_all();
    }

    fn checked(&mut self, verdict: Verdict) {
        if self.entry.checked(verdict) {
            self.unlock = true;
            self.try_unlock();
        } else {
            self.draw_all();
        }
    }

    /// Unlocks once PAM has accepted the password and the compositor has confirmed the lock.
    fn try_unlock(&mut self) {
        if !(self.unlock && self.locked) {
            return;
        }
        if let Some(lock) = self.lock.take() {
            lock.unlock();
        }
        // the lock surfaces go after the unlock request, as the protocol asks
        self.screens.clear();
        if let Err(e) = self.connection.roundtrip() {
            eprintln!("umbra-lock: the compositor did not confirm the unlock: {e}");
        }
        self.done = Some(Ok(()));
    }
}

impl SessionLockHandler for LockScreen {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        self.locked = true;
        self.try_unlock();
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        self.done = Some(if self.locked {
            Ok(())
        } else {
            Err("The compositor did not lock the session. It may be locked already.".to_string())
        });
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        self.screens[index].size = Some(configure.new_size);
        self.draw(index);
    }
}

impl CompositorHandler for LockScreen {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface)
        else {
            return;
        };
        if self.screens[index].scale != new_factor {
            self.screens[index].scale = new_factor;
            self.draw(index);
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

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

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

impl OutputHandler for LockScreen {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_screen(qh, output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.screens.retain(|screen| screen.output != output);
    }
}

impl SeatHandler for LockScreen {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard || self.keyboard.is_some() {
            return;
        }
        let repeat = Box::new(
            |lock: &mut Self, _: &wl_keyboard::WlKeyboard, event: KeyEvent| {
                lock.press(&event);
            },
        );
        match self
            .seats
            .get_keyboard_with_repeat(qh, &seat, None, self.loop_handle.clone(), repeat)
        {
            Ok(keyboard) => self.keyboard = Some(keyboard),
            Err(e) => eprintln!("umbra-lock: could not use the keyboard: {e}"),
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for LockScreen {
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
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.press(&event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.press(&event);
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
        self.ctrl = modifiers.ctrl;
    }
}

impl ProvidesRegistryState for LockScreen {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

impl ShmHandler for LockScreen {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(LockScreen);
delegate_output!(LockScreen);
delegate_seat!(LockScreen);
delegate_keyboard!(LockScreen);
delegate_session_lock!(LockScreen);
delegate_shm!(LockScreen);
delegate_registry!(LockScreen);
