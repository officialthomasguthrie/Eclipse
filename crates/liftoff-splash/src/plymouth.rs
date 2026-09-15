//! The plugin plymouthd loads: the table of functions plymouth calls, and the few of plymouth's it
//! calls back. plymouth's libraries are already in plymouthd, so their functions are looked up by
//! name when the plugin is created instead of linked, and the crate builds and tests without them.
//!
//! plymouth hands the plugin its displays, every byte written to the console, and the questions it
//! asks. The plugin keeps the console, and draws a frame on a timer when something changed. A panic
//! stops at the edge of each function plymouth calls: plymouthd goes on, and so does the passphrase
//! prompt it asks systemd's question with.

#![allow(
    unsafe_code,
    reason = "plymouth calls the plugin through a table of C functions and hands it C pointers"
)]

use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use crate::console::Console;
use crate::font::Fonts;
use crate::logo::Logo;
use crate::screen::{Banner, Frame, Prompt, View};

/// How long the plugin waits after a change before it draws, so a burst of output is one frame.
const FRAME: f64 = 1.0 / 30.0;
/// A cell's width at scale 1.
const CELL_WIDTH: usize = 8;

type Pointer = *mut c_void;
type DrawHandler = unsafe extern "C" fn(Pointer, Pointer, c_int, c_int, c_int, c_int, Pointer);
type TimeoutHandler = unsafe extern "C" fn(Pointer, Pointer);
type ExitHandler = unsafe extern "C" fn(Pointer, c_int, Pointer);

/// plymouth's `ply_rectangle_t`.
#[repr(C)]
struct Rectangle {
    x: c_long,
    y: c_long,
    width: c_ulong,
    height: c_ulong,
}

/// plymouth's functions the plugin calls.
#[derive(Clone, Copy)]
struct Plymouth {
    display_width: unsafe extern "C" fn(Pointer) -> c_ulong,
    display_height: unsafe extern "C" fn(Pointer) -> c_ulong,
    display_scale: unsafe extern "C" fn(Pointer) -> c_int,
    set_draw_handler: unsafe extern "C" fn(Pointer, Option<DrawHandler>, Pointer),
    draw_area: unsafe extern "C" fn(Pointer, c_int, c_int, c_int, c_int),
    fill: unsafe extern "C" fn(Pointer, *mut Rectangle, *mut Rectangle, *mut u32, f64, c_int),
    watch_timeout: unsafe extern "C" fn(Pointer, f64, TimeoutHandler, Pointer),
    stop_timeout: unsafe extern "C" fn(Pointer, TimeoutHandler, Pointer),
    watch_exit: unsafe extern "C" fn(Pointer, ExitHandler, Pointer),
    stop_exit: unsafe extern "C" fn(Pointer, ExitHandler, Pointer),
    key_file_value: unsafe extern "C" fn(Pointer, *const c_char, *const c_char) -> *mut c_char,
    pull_trigger: unsafe extern "C" fn(Pointer, *const c_void),
    buffer_bytes: unsafe extern "C" fn(Pointer) -> *const c_char,
    buffer_size: unsafe extern "C" fn(Pointer) -> usize,
}

/// Looks up one of plymouth's functions in plymouthd.
///
/// # Safety
///
/// `F` has to be the type of the function with that name.
unsafe fn look_up<F: Copy>(name: &CStr) -> Option<F> {
    // SAFETY: a nul terminated name, looked up in what plymouthd has loaded
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    if found.is_null() || size_of::<F>() != size_of::<Pointer>() {
        return None;
    }
    // SAFETY: F is the function's pointer type, the caller says, and it is a pointer's size
    Some(unsafe { std::mem::transmute_copy::<Pointer, F>(&found) })
}

impl Plymouth {
    /// plymouth's functions, or none when one of them is missing.
    fn look_up() -> Option<Self> {
        // SAFETY: each type is the one plymouth 26's headers declare for the name
        unsafe {
            Some(Self {
                display_width: look_up(c"ply_pixel_display_get_width")?,
                display_height: look_up(c"ply_pixel_display_get_height")?,
                display_scale: look_up(c"ply_pixel_display_get_device_scale")?,
                set_draw_handler: look_up(c"ply_pixel_display_set_draw_handler")?,
                draw_area: look_up(c"ply_pixel_display_draw_area")?,
                fill: look_up(
                    c"ply_pixel_buffer_fill_with_argb32_data_at_opacity_with_clip_and_scale",
                )?,
                watch_timeout: look_up(c"ply_event_loop_watch_for_timeout")?,
                stop_timeout: look_up(c"ply_event_loop_stop_watching_for_timeout")?,
                watch_exit: look_up(c"ply_event_loop_watch_for_exit")?,
                stop_exit: look_up(c"ply_event_loop_stop_watching_for_exit")?,
                key_file_value: look_up(c"ply_key_file_get_value")?,
                pull_trigger: look_up(c"ply_trigger_pull")?,
                buffer_bytes: look_up(c"ply_buffer_get_bytes")?,
                buffer_size: look_up(c"ply_buffer_get_size")?,
            })
        }
    }

    /// A value from the theme file's [liftoff] group.
    ///
    /// # Safety
    ///
    /// `key_file` is the key file plymouth handed `create_plugin`, or null.
    unsafe fn value(&self, key_file: Pointer, key: &CStr) -> Option<String> {
        if key_file.is_null() {
            return None;
        }
        // SAFETY: the theme's key file and two nul terminated names
        let value = unsafe { (self.key_file_value)(key_file, c"liftoff".as_ptr(), key.as_ptr()) };
        if value.is_null() {
            return None;
        }
        // SAFETY: plymouth returns a nul terminated copy that the caller frees
        let text = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: the copy came from malloc
        unsafe { libc::free(value.cast()) };
        Some(text)
    }

    /// A display's size in pixels, and its scale.
    ///
    /// # Safety
    ///
    /// `display` is a display plymouth handed the plugin.
    unsafe fn size(&self, display: Pointer) -> (usize, usize, usize) {
        // SAFETY: the caller's display
        let (width, height, scale) = unsafe {
            (
                (self.display_width)(display),
                (self.display_height)(display),
                (self.display_scale)(display),
            )
        };
        let scale = usize::try_from(scale).unwrap_or(1).clamp(1, 4);
        let pixels = |logical: c_ulong| usize::try_from(logical).unwrap_or(0).saturating_mul(scale);
        (pixels(width), pixels(height), scale)
    }
}

/// A display and the frame drawn for it.
struct Display {
    handle: Pointer,
    frame: Frame,
}

/// What the plugin keeps between plymouth's calls.
pub struct Plugin {
    plymouth: Option<Plymouth>,
    title: String,
    faces: Option<(Vec<u8>, Vec<u8>)>,
    fonts: Vec<Fonts>,
    logo: Logo,
    console: Console,
    prompt: Option<Prompt>,
    details: bool,
    event_loop: Pointer,
    displays: Vec<Display>,
    shown: bool,
    queued: bool,
}

impl Plugin {
    fn new(key_file: Pointer) -> Self {
        let plymouth = Plymouth::look_up();
        // SAFETY: the key file plymouth handed create_plugin
        let value = |key: &CStr| {
            plymouth
                .as_ref()
                .and_then(|plymouth| unsafe { plymouth.value(key_file, key) })
        };
        let title = value(c"Title").unwrap_or_else(|| "Rift".to_string());
        let read = |key: &CStr| value(key).and_then(|path| std::fs::read(path).ok());
        let faces = read(c"Font").zip(read(c"BoldFont"));
        Self {
            plymouth,
            title,
            faces,
            fonts: Vec::new(),
            logo: Logo::new(),
            console: Console::new(),
            prompt: None,
            details: false,
            event_loop: ptr::null_mut(),
            displays: Vec::new(),
            shown: false,
            queued: false,
        }
    }

    fn this(&mut self) -> Pointer {
        ptr::from_mut(self).cast()
    }

    /// Asks the event loop to draw in a moment.
    fn queue(&mut self) {
        if !self.shown || self.queued || self.event_loop.is_null() {
            return;
        }
        let Some(plymouth) = self.plymouth else {
            return;
        };
        let this = self.this();
        // SAFETY: the loop plymouth showed the splash on, which has not exited
        unsafe { (plymouth.watch_timeout)(self.event_loop, FRAME, on_timeout, this) };
        self.queued = true;
    }

    /// Stops drawing: no timer, no draw handlers.
    fn stop(&mut self) {
        let this = self.this();
        if let Some(plymouth) = self.plymouth {
            if !self.event_loop.is_null() {
                // SAFETY: the loop is still there, its exit handler would have cleared it
                unsafe {
                    (plymouth.stop_timeout)(self.event_loop, on_timeout, this);
                    (plymouth.stop_exit)(self.event_loop, on_loop_exit, this);
                }
            }
            for display in &self.displays {
                // SAFETY: a display plymouth added and has not removed
                unsafe { (plymouth.set_draw_handler)(display.handle, None, ptr::null_mut()) };
            }
        }
        self.event_loop = ptr::null_mut();
        self.shown = false;
        self.queued = false;
    }

    /// Draws the frame for a display into the buffer plymouth draws it with.
    fn draw(&mut self, buffer: Pointer, handle: Pointer) {
        let Some(plymouth) = self.plymouth else {
            return;
        };
        let Some(index) = self
            .displays
            .iter()
            .position(|display| display.handle == handle)
        else {
            return;
        };
        // SAFETY: the display plymouth is drawing
        let (width, height, scale) = unsafe { plymouth.size(handle) };
        let cell_width = CELL_WIDTH * scale;
        if !self
            .fonts
            .iter()
            .any(|fonts| fonts.cell_width() == cell_width)
        {
            let Some((regular, bold)) = &self.faces else {
                return;
            };
            let Ok(fonts) = Fonts::new(regular.clone(), bold.clone(), cell_width) else {
                return;
            };
            self.fonts.push(fonts);
        }
        let Self {
            fonts,
            logo,
            console,
            prompt,
            details,
            title,
            displays,
            ..
        } = self;
        let (Some(fonts), Some(display)) = (
            fonts
                .iter_mut()
                .find(|fonts| fonts.cell_width() == cell_width),
            displays.get_mut(index),
        ) else {
            return;
        };
        let frame = &mut display.frame;
        frame.resize(width, height);
        let view = View {
            console,
            prompt: prompt.as_ref(),
            title,
            details: *details,
            banner: Banner::Under,
        };
        frame.draw(fonts, logo, &view);
        let mut area = Rectangle {
            x: 0,
            y: 0,
            width: c_ulong::try_from(width).unwrap_or(0),
            height: c_ulong::try_from(height).unwrap_or(0),
        };
        let scale = c_int::try_from(scale).unwrap_or(1);
        // SAFETY: the buffer plymouth handed the draw handler and a frame of the area's size, in
        // pixels of the display's scale
        unsafe {
            (plymouth.fill)(
                buffer,
                &raw mut area,
                ptr::null_mut(),
                frame.pixels_mut().as_mut_ptr(),
                1.0,
                scale,
            );
        }
    }
}

/// Runs the body of a function plymouth called, with the plugin it passed. A panic ends there.
fn with<R>(plugin: *mut Plugin, otherwise: R, body: impl FnOnce(&mut Plugin) -> R) -> R {
    // SAFETY: plymouth passes back the pointer create_plugin returned, until destroy_plugin
    let Some(plugin) = (unsafe { plugin.as_mut() }) else {
        return otherwise;
    };
    catch_unwind(AssertUnwindSafe(|| body(plugin))).unwrap_or(otherwise)
}

/// A string plymouth passed.
///
/// # Safety
///
/// `text` is null or nul terminated.
unsafe fn text(text: *const c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the caller's nul terminated string
    Some(
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned(),
    )
}

unsafe extern "C" fn create_plugin(key_file: Pointer) -> *mut Plugin {
    // plymouth asserts that it gets a plugin. one that lacks plymouth's functions or its fonts
    // refuses to show instead
    let plugin =
        catch_unwind(|| Plugin::new(key_file)).unwrap_or_else(|_| Plugin::new(ptr::null_mut()));
    Box::into_raw(Box::new(plugin))
}

unsafe extern "C" fn destroy_plugin(plugin: *mut Plugin) {
    if plugin.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the box create_plugin made, which plymouth gives back once
        let mut plugin = unsafe { Box::from_raw(plugin) };
        plugin.stop();
    }));
}

unsafe extern "C" fn add_pixel_display(plugin: *mut Plugin, display: Pointer) {
    with(plugin, (), |plugin| {
        if plugin.displays.iter().any(|known| known.handle == display) {
            return;
        }
        plugin.displays.push(Display {
            handle: display,
            frame: Frame::default(),
        });
        if plugin.shown {
            let this = plugin.this();
            if let Some(plymouth) = plugin.plymouth {
                // SAFETY: the display plymouth just added
                unsafe { (plymouth.set_draw_handler)(display, Some(on_draw), this) };
            }
            plugin.queue();
        }
    });
}

unsafe extern "C" fn remove_pixel_display(plugin: *mut Plugin, display: Pointer) {
    with(plugin, (), |plugin| {
        if let Some(plymouth) = plugin.plymouth {
            // SAFETY: a display plymouth is removing
            unsafe { (plymouth.set_draw_handler)(display, None, ptr::null_mut()) };
        }
        plugin.displays.retain(|known| known.handle != display);
    });
}

unsafe extern "C" fn show_splash_screen(
    plugin: *mut Plugin,
    event_loop: Pointer,
    boot_buffer: Pointer,
    _mode: c_int,
) -> bool {
    with(plugin, false, |plugin| {
        let Some(plymouth) = plugin.plymouth else {
            return false;
        };
        if plugin.faces.is_none() || plugin.displays.is_empty() || event_loop.is_null() {
            return false;
        }
        plugin.console = Console::new();
        if !boot_buffer.is_null() {
            // SAFETY: plymouth's buffer of what was written to the console so far
            unsafe {
                let bytes = (plymouth.buffer_bytes)(boot_buffer);
                let size = (plymouth.buffer_size)(boot_buffer);
                if !bytes.is_null() {
                    plugin
                        .console
                        .write(std::slice::from_raw_parts(bytes.cast(), size));
                }
            }
        }
        let this = plugin.this();
        plugin.event_loop = event_loop;
        // SAFETY: the loop plymouth runs, and the displays it added
        unsafe {
            (plymouth.watch_exit)(event_loop, on_loop_exit, this);
            for display in &plugin.displays {
                (plymouth.set_draw_handler)(display.handle, Some(on_draw), this);
            }
        }
        plugin.shown = true;
        plugin.queue();
        true
    })
}

unsafe extern "C" fn hide_splash_screen(plugin: *mut Plugin, _event_loop: Pointer) {
    with(plugin, (), Plugin::stop);
}

unsafe extern "C" fn update_status(_plugin: *mut Plugin, _status: *const c_char) {}

unsafe extern "C" fn on_boot_output(plugin: *mut Plugin, output: *const c_char, size: usize) {
    with(plugin, (), |plugin| {
        if !output.is_null() {
            // SAFETY: plymouth passes `size` bytes of console output
            plugin
                .console
                .write(unsafe { std::slice::from_raw_parts(output.cast(), size) });
        }
        plugin.queue();
    });
}

unsafe extern "C" fn display_message(plugin: *mut Plugin, message: *const c_char) {
    with(plugin, (), |plugin| {
        // SAFETY: plymouth's nul terminated message
        if let Some(message) = unsafe { text(message) } {
            plugin.console.message(&message);
            plugin.queue();
        }
    });
}

unsafe extern "C" fn display_normal(plugin: *mut Plugin) {
    with(plugin, (), |plugin| {
        plugin.prompt = None;
        plugin.queue();
    });
}

unsafe extern "C" fn display_password(plugin: *mut Plugin, prompt: *const c_char, bullets: c_int) {
    with(plugin, (), |plugin| {
        plugin.prompt = Some(Prompt::Secret {
            // SAFETY: plymouth's nul terminated prompt
            question: unsafe { text(prompt) }.unwrap_or_else(|| "Passphrase".to_string()),
            typed: usize::try_from(bullets).unwrap_or(0),
        });
        plugin.queue();
    });
}

unsafe extern "C" fn display_question(
    plugin: *mut Plugin,
    prompt: *const c_char,
    entry: *const c_char,
) {
    with(plugin, (), |plugin| {
        plugin.prompt = Some(Prompt::Visible {
            // SAFETY: plymouth's nul terminated prompt and answer
            question: unsafe { text(prompt) }.unwrap_or_default(),
            answer: unsafe { text(entry) }.unwrap_or_default(),
        });
        plugin.queue();
    });
}

unsafe extern "C" fn become_idle(plugin: *mut Plugin, trigger: Pointer) {
    with(plugin, (), |plugin| {
        if let Some(plymouth) = plugin.plymouth
            && !trigger.is_null()
        {
            // SAFETY: the trigger plymouth waits on
            unsafe { (plymouth.pull_trigger)(trigger, ptr::null()) };
        }
    });
}

unsafe extern "C" fn validate_input(
    plugin: *mut Plugin,
    _entry: *const c_char,
    added: *const c_char,
) -> bool {
    with(plugin, true, |plugin| {
        // SAFETY: plymouth's nul terminated key
        if unsafe { text(added) }.as_deref() != Some("\u{1b}") {
            return true;
        }
        // Esc switches between the boot and the details view, the console alone
        plugin.details = !plugin.details;
        plugin.queue();
        false
    })
}

unsafe extern "C" fn on_draw(
    plugin: Pointer,
    buffer: Pointer,
    _x: c_int,
    _y: c_int,
    _width: c_int,
    _height: c_int,
    display: Pointer,
) {
    with(plugin.cast(), (), |plugin| plugin.draw(buffer, display));
}

unsafe extern "C" fn on_timeout(plugin: Pointer, _event_loop: Pointer) {
    // the displays to draw are gathered first: drawing calls back into on_draw, which takes the plugin
    // again
    let areas = with(plugin.cast::<Plugin>(), None, |plugin| {
        plugin.queued = false;
        let plymouth = plugin.plymouth.filter(|_| plugin.shown)?;
        let areas: Vec<(Pointer, c_int, c_int)> = plugin
            .displays
            .iter()
            .map(|display| {
                // SAFETY: a display plymouth added
                let (width, height, scale) = unsafe { plymouth.size(display.handle) };
                let logical = |pixels: usize| c_int::try_from(pixels / scale).unwrap_or(0);
                (display.handle, logical(width), logical(height))
            })
            .collect();
        Some((plymouth.draw_area, areas))
    });
    if let Some((draw_area, areas)) = areas {
        for (display, width, height) in areas {
            // SAFETY: a display plymouth added, in its own coordinates
            unsafe { draw_area(display, 0, 0, width, height) };
        }
    }
}

unsafe extern "C" fn on_loop_exit(plugin: Pointer, _code: c_int, _event_loop: Pointer) {
    with(plugin.cast::<Plugin>(), (), |plugin| {
        plugin.event_loop = ptr::null_mut();
        plugin.queued = false;
    });
}

/// plymouth's `ply_boot_splash_plugin_interface_t`, in the order its header declares it.
#[repr(C)]
pub struct Interface {
    create_plugin: Option<unsafe extern "C" fn(Pointer) -> *mut Plugin>,
    destroy_plugin: Option<unsafe extern "C" fn(*mut Plugin)>,
    set_keyboard: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    unset_keyboard: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    add_pixel_display: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    remove_pixel_display: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    add_text_display: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    remove_text_display: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    show_splash_screen: Option<unsafe extern "C" fn(*mut Plugin, Pointer, Pointer, c_int) -> bool>,
    system_update: Option<unsafe extern "C" fn(*mut Plugin, c_int)>,
    update_status: Option<unsafe extern "C" fn(*mut Plugin, *const c_char)>,
    on_boot_output: Option<unsafe extern "C" fn(*mut Plugin, *const c_char, usize)>,
    on_boot_progress: Option<unsafe extern "C" fn(*mut Plugin, f64, f64)>,
    on_root_mounted: Option<unsafe extern "C" fn(*mut Plugin)>,
    hide_splash_screen: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    display_message: Option<unsafe extern "C" fn(*mut Plugin, *const c_char)>,
    hide_message: Option<unsafe extern "C" fn(*mut Plugin, *const c_char)>,
    display_normal: Option<unsafe extern "C" fn(*mut Plugin)>,
    display_password: Option<unsafe extern "C" fn(*mut Plugin, *const c_char, c_int)>,
    display_question: Option<unsafe extern "C" fn(*mut Plugin, *const c_char, *const c_char)>,
    become_idle: Option<unsafe extern "C" fn(*mut Plugin, Pointer)>,
    display_prompt: Option<unsafe extern "C" fn(*mut Plugin, *const c_char, *const c_char, bool)>,
    validate_input: Option<unsafe extern "C" fn(*mut Plugin, *const c_char, *const c_char) -> bool>,
}

static INTERFACE: Interface = Interface {
    create_plugin: Some(create_plugin),
    destroy_plugin: Some(destroy_plugin),
    set_keyboard: None,
    unset_keyboard: None,
    add_pixel_display: Some(add_pixel_display),
    remove_pixel_display: Some(remove_pixel_display),
    add_text_display: None,
    remove_text_display: None,
    show_splash_screen: Some(show_splash_screen),
    system_update: None,
    update_status: Some(update_status),
    on_boot_output: Some(on_boot_output),
    on_boot_progress: None,
    on_root_mounted: None,
    hide_splash_screen: Some(hide_splash_screen),
    display_message: Some(display_message),
    hide_message: None,
    display_normal: Some(display_normal),
    display_password: Some(display_password),
    display_question: Some(display_question),
    become_idle: Some(become_idle),
    // plymouth calls it right after display_password and display_question with the same question
    display_prompt: None,
    validate_input: Some(validate_input),
};

/// The function plymouthd looks up in a splash plugin.
#[unsafe(no_mangle)]
pub extern "C" fn ply_boot_splash_plugin_get_interface() -> *const Interface {
    &raw const INTERFACE
}
