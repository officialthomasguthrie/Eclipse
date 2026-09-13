//! The window: the image, the disks eclipse-flash lists, the passphrase on Linux and the serial or name
//! typed back, then the steps eclipse-flash prints while it writes. Drawn with iced in software in a
//! normal window, with Noto Sans built in, in the light or dark colors the system uses.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eclipse_flash::Listed;
use iced::futures::channel::{mpsc, oneshot};
use iced::widget::{
    button, column, container, mouse_area, radio, row, rule, scrollable, space, text, text_input,
};
use iced::{
    Border, Center, Color, Element, Fill, Font, Size, Subscription, Task, Theme, event, font,
    system, theme, window,
};
use libeclipse::disk::passphrase_problem;

use crate::fonts;
use crate::rights::{self, Event, Job};

const TEXT_SIZE: f32 = 14.0;
/// One step above the text, for the title of the steps.
const TITLE_SIZE: f32 = 17.0;
const RADIO_SIZE: f32 = 16.0;
const FONT: Font = Font::with_name("Noto Sans");
const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..FONT
};

/// What the app is started with.
#[derive(Debug, Default)]
pub struct Start {
    /// The image, when one was given.
    pub image: Option<PathBuf>,
    /// Where to save a picture of the window once the disks are listed, and then quit.
    pub screenshot: Option<PathBuf>,
}

/// The colors of one theme.
#[derive(Debug, Clone, Copy)]
struct Colors {
    window: Color,
    field: Color,
    button: Color,
    text: Color,
    dim: Color,
    line: Color,
    edge: Color,
    accent: Color,
    on_accent: Color,
    error: Color,
}

const LIGHT: Colors = Colors {
    window: Color::from_rgb8(0xfa, 0xfa, 0xfa),
    field: Color::from_rgb8(0xff, 0xff, 0xff),
    button: Color::from_rgb8(0xeb, 0xeb, 0xeb),
    text: Color::from_rgb8(0x24, 0x24, 0x24),
    dim: Color::from_rgb8(0x6e, 0x6e, 0x6e),
    line: Color::from_rgb8(0xde, 0xde, 0xde),
    edge: Color::from_rgb8(0xc4, 0xc4, 0xc4),
    accent: Color::from_rgb8(0x35, 0x84, 0xe4),
    on_accent: Color::from_rgb8(0xff, 0xff, 0xff),
    error: Color::from_rgb8(0xc0, 0x1c, 0x28),
};

const DARK: Colors = Colors {
    window: Color::from_rgb8(0x24, 0x24, 0x24),
    field: Color::from_rgb8(0x1e, 0x1e, 0x1e),
    button: Color::from_rgb8(0x2e, 0x2e, 0x2e),
    text: Color::from_rgb8(0xe6, 0xe6, 0xe6),
    dim: Color::from_rgb8(0x9a, 0x9a, 0x9a),
    line: Color::from_rgb8(0x38, 0x38, 0x38),
    edge: Color::from_rgb8(0x4a, 0x4a, 0x4a),
    accent: Color::from_rgb8(0x78, 0xae, 0xed),
    on_accent: Color::from_rgb8(0x1e, 0x1e, 0x1e),
    error: Color::from_rgb8(0xe0, 0x6d, 0x6d),
};

struct Flash {
    image: Option<Chosen>,
    disks: Disks,
    /// The path of the disk that is selected.
    selected: Option<String>,
    typed: String,
    /// Whether persist is left for the drive to make when it first starts. Always, but on Linux.
    later: bool,
    passphrase: String,
    again: String,
    /// eclipse-flash, or why it is not found.
    program: Result<PathBuf, String>,
    writing: Option<Writing>,
    mode: theme::Mode,
    screenshot: Option<PathBuf>,
    shot: bool,
}

/// An image that was chosen, and its version or why it cannot be written.
struct Chosen {
    path: PathBuf,
    version: Result<String, String>,
}

enum Disks {
    Reading,
    Read(Vec<Listed>),
    Failed(String),
}

/// A write that was started, and the lines eclipse-flash printed so far.
struct Writing {
    disk: String,
    lines: Vec<Line>,
    ended: Option<bool>,
    /// Whether the window was asked to close while it wrote.
    closing: bool,
}

enum Line {
    Said(String),
    Failed(String),
}

#[derive(Debug, Clone)]
enum Message {
    Choose,
    Chosen(Option<PathBuf>),
    Dropped(PathBuf),
    Refresh,
    Disks(Result<Vec<Listed>, String>),
    Select(usize),
    Type(String),
    Later(bool),
    Passphrase(String),
    Again(String),
    Write,
    Wrote(Event),
    Done,
    Mode(theme::Mode),
    Close,
    Shot(window::Screenshot),
}

/// Opens the window and runs until it is closed.
///
/// # Errors
///
/// When the window cannot be opened.
pub fn run(start: Start) -> iced::Result {
    iced::application(move || boot(&start), update, view)
        .title("Eclipse flash")
        .theme(|state: &Flash| {
            Theme::custom(
                "Eclipse",
                theme::Palette {
                    background: state.colors().window,
                    text: state.colors().text,
                    primary: state.colors().accent,
                    success: Color::from_rgb8(0x2e, 0xc2, 0x7e),
                    warning: Color::from_rgb8(0xe5, 0xa5, 0x0a),
                    danger: state.colors().error,
                },
            )
        })
        .subscription(subscription)
        .font(fonts::regular())
        .font(fonts::bold())
        .default_font(FONT)
        .window(window::Settings {
            size: Size::new(640.0, 720.0),
            min_size: Some(Size::new(480.0, 440.0)),
            exit_on_close_request: false,
            ..window::Settings::default()
        })
        .run()
}

fn boot(start: &Start) -> (Flash, Task<Message>) {
    let state = Flash {
        image: start.image.as_deref().map(chosen),
        disks: Disks::Reading,
        selected: None,
        typed: String::new(),
        later: !cfg!(target_os = "linux"),
        passphrase: String::new(),
        again: String::new(),
        program: program(),
        writing: None,
        mode: theme::Mode::None,
        screenshot: start.screenshot.clone(),
        shot: false,
    };
    (
        state,
        Task::batch([read_disks(), system::theme().map(Message::Mode)]),
    )
}

fn subscription(_: &Flash) -> Subscription<Message> {
    Subscription::batch([
        event::listen_with(|event, _, _| match event {
            iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::Dropped(path)),
            _ => None,
        }),
        window::close_requests().map(|_| Message::Close),
        system::theme_changes().map(Message::Mode),
    ])
}

/// eclipse-flash in the folder the app is in, or on the path.
fn program() -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "eclipse-flash.exe"
    } else {
        "eclipse-flash"
    };
    let beside = std::env::current_exe()
        .ok()
        .and_then(|app| app.parent().map(|folder| folder.join(name)));
    let on_path = || {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|folder| folder.join(name))
                .find(|program| program.is_file())
        })
    };
    beside
        .filter(|program| program.is_file())
        .or_else(on_path)
        .ok_or_else(|| {
            format!(
                "{name} is not in the folder this app is in. The app starts it to write the drive, so keep the two together."
            )
        })
}

fn chosen(path: &Path) -> Chosen {
    Chosen {
        path: path.to_path_buf(),
        version: eclipse_flash::version(path),
    }
}

/// Lists the disks on a thread of its own: diskutil and PowerShell take a moment.
fn read_disks() -> Task<Message> {
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(eclipse_flash::disks());
    });
    Task::perform(receiver, |read| {
        Message::Disks(read.unwrap_or_else(|_| Err("Could not list the disks.".into())))
    })
}

impl Flash {
    fn colors(&self) -> Colors {
        match self.mode {
            theme::Mode::Dark => DARK,
            theme::Mode::Light | theme::Mode::None => LIGHT,
        }
    }

    /// The disk that is selected, while it is listed and passes.
    fn disk(&self) -> Option<&Listed> {
        let Disks::Read(listed) = &self.disks else {
            return None;
        };
        let path = self.selected.as_deref()?;
        listed
            .iter()
            .find(|item| item.disk.path == path && item.refused.is_none())
    }

    /// What is wrong with the passphrase typed so far, once something is typed.
    fn passphrase_problem(&self) -> Option<&'static str> {
        if self.later || self.passphrase.is_empty() {
            return None;
        }
        passphrase_problem(&self.passphrase).or_else(|| {
            (!self.again.is_empty() && self.again != self.passphrase)
                .then_some("The two passphrases are not the same.")
        })
    }

    /// The write, once everything it needs is chosen and typed.
    fn job(&self) -> Option<Job> {
        let image = self.image.as_ref().filter(|image| image.version.is_ok())?;
        let listed = self.disk()?;
        let program = self.program.as_ref().ok()?;
        if self.typed.trim() != listed.disk.confirmation() {
            return None;
        }
        let passphrase = if self.later {
            None
        } else if passphrase_problem(&self.passphrase).is_none() && self.again == self.passphrase {
            Some(self.passphrase.clone())
        } else {
            return None;
        };
        Some(Job {
            program: program.clone(),
            image: image.path.clone(),
            disk: listed.disk.path.clone(),
            serial: self.typed.trim().to_string(),
            passphrase,
        })
    }
}

fn update(state: &mut Flash, message: Message) -> Task<Message> {
    match message {
        Message::Choose => Task::perform(
            rfd::AsyncFileDialog::new()
                .set_title("Choose an Eclipse image")
                .add_filter("Eclipse image", &["raw", "zst"])
                .pick_file(),
            |file| Message::Chosen(file.map(|file| file.path().to_path_buf())),
        ),
        Message::Chosen(Some(path)) | Message::Dropped(path) => {
            if state.writing.is_none() {
                state.image = Some(chosen(&path));
            }
            Task::none()
        }
        Message::Refresh => {
            state.disks = Disks::Reading;
            read_disks()
        }
        Message::Disks(read) => {
            state.disks = match read {
                Ok(listed) => Disks::Read(listed),
                Err(why) => Disks::Failed(why),
            };
            if state.disk().is_none() {
                state.selected = None;
                state.typed.clear();
            }
            if state.screenshot.is_some() && !state.shot {
                state.shot = true;
                return shoot(state);
            }
            Task::none()
        }
        Message::Select(index) => {
            if let Disks::Read(listed) = &state.disks
                && let Some(item) = listed.get(index).filter(|item| item.refused.is_none())
                && state.selected.as_deref() != Some(item.disk.path.as_str())
            {
                state.selected = Some(item.disk.path.clone());
                state.typed.clear();
            }
            Task::none()
        }
        Message::Type(typed) => {
            state.typed = typed;
            Task::none()
        }
        Message::Later(later) => {
            state.later = later;
            Task::none()
        }
        Message::Passphrase(passphrase) => {
            state.passphrase = passphrase;
            Task::none()
        }
        Message::Again(again) => {
            state.again = again;
            Task::none()
        }
        Message::Write => write(state),
        Message::Wrote(event) => {
            if let Some(writing) = &mut state.writing {
                writing.take(event);
            }
            Task::none()
        }
        Message::Done => {
            state.writing = None;
            state.disks = Disks::Reading;
            read_disks()
        }
        Message::Mode(mode) => {
            state.mode = mode;
            Task::none()
        }
        Message::Close => match &mut state.writing {
            Some(writing) if writing.ended.is_none() => {
                writing.closing = true;
                Task::none()
            }
            _ => iced::exit(),
        },
        Message::Shot(shot) => {
            if let Some(path) = &state.screenshot
                && let Err(why) = save(path, &shot)
            {
                eprintln!("eclipse-flash-app: {why}");
            }
            iced::exit()
        }
        Message::Chosen(None) => Task::none(),
    }
}

/// Starts eclipse-flash with the rights to write, and forgets the passphrase and what was typed.
fn write(state: &mut Flash) -> Task<Message> {
    let Some(job) = state.job() else {
        return Task::none();
    };
    state.writing = Some(Writing {
        disk: job.disk.clone(),
        lines: Vec::new(),
        ended: None,
        closing: false,
    });
    state.selected = None;
    state.typed.clear();
    state.passphrase.clear();
    state.again.clear();
    let (sender, receiver) = mpsc::unbounded();
    rights::start(job, sender);
    Task::run(receiver, Message::Wrote)
}

impl Writing {
    fn take(&mut self, event: Event) {
        // how far a copy or a read back is, which the next such line says again
        let progress = |line: &str| line.starts_with("Copied ") || line.starts_with("Checked ");
        match event {
            Event::Said(line) => {
                if progress(&line)
                    && matches!(self.lines.last(), Some(Line::Said(last)) if progress(last))
                {
                    self.lines.pop();
                }
                self.lines.push(Line::Said(line));
            }
            Event::Failed(line) => {
                if !line.trim().is_empty() {
                    self.lines.push(Line::Failed(line));
                }
            }
            Event::Ended(written) => self.ended = Some(written),
        }
    }
}

/// For --screenshot: selects the first disk that passes, types its serial or name, waits a moment
/// for the window to show it, and takes the picture.
fn shoot(state: &mut Flash) -> Task<Message> {
    if let Disks::Read(listed) = &state.disks
        && let Some(item) = listed.iter().find(|item| item.refused.is_none())
    {
        state.selected = Some(item.disk.path.clone());
        state.typed = item.disk.confirmation().to_string();
    }
    Task::perform(
        async { std::thread::sleep(Duration::from_millis(800)) },
        |()| (),
    )
    .then(|()| window::oldest())
    .and_then(window::screenshot)
    .map(Message::Shot)
}

fn save(path: &Path, shot: &window::Screenshot) -> Result<(), String> {
    let writing = |e: String| format!("Could not write {}: {e}", path.display());
    let file = File::create(path).map_err(|e| writing(e.to_string()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), shot.size.width, shot.size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut png| png.write_image_data(&shot.rgba))
        .map_err(|e| writing(e.to_string()))
}

fn view(state: &Flash) -> Element<'_, Message> {
    let colors = state.colors();
    let page = match &state.writing {
        Some(writing) => steps(writing, colors),
        None => choose(state, colors),
    };
    container(page)
        .width(Fill)
        .height(Fill)
        .style(move |_| container::Style {
            background: Some(colors.window.into()),
            text_color: Some(colors.text),
            ..container::Style::default()
        })
        .into()
}

/// The page where the image, the disk and the passphrase are chosen.
fn choose(state: &Flash, colors: Colors) -> Element<'_, Message> {
    let mut page = column![image(state, colors), line(colors), drive(state, colors)].spacing(16);
    if cfg!(target_os = "linux") {
        page = page.push(line(colors)).push(persist(state, colors));
    }
    page = page.push(line(colors)).push(confirm(state, colors));
    scrollable(container(page.padding(24).max_width(640)).center_x(Fill))
        .height(Fill)
        .into()
}

fn image(state: &Flash, colors: Colors) -> Element<'_, Message> {
    let (name, about) = match &state.image {
        None => (
            "No image chosen".to_string(),
            note(
                "A .raw or .raw.zst file. You can also drop it onto this window.",
                colors.dim,
            ),
        ),
        Some(chosen) => (
            chosen.path.file_name().map_or_else(
                || chosen.path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            match &chosen.version {
                Ok(version) => note(
                    format!(
                        "Eclipse {version}, in {}.",
                        chosen
                            .path
                            .parent()
                            .map_or_else(String::new, |folder| folder.display().to_string())
                    ),
                    colors.dim,
                ),
                Err(why) => note(why.clone(), colors.error),
            },
        ),
    };
    column![
        heading("Image"),
        row![
            column![text(name).size(TEXT_SIZE), about]
                .spacing(2)
                .width(Fill),
            plain(
                "Choose",
                (state.writing.is_none()).then_some(Message::Choose),
                colors
            ),
        ]
        .spacing(12)
        .align_y(Center),
    ]
    .spacing(8)
    .into()
}

fn drive(state: &Flash, colors: Colors) -> Element<'_, Message> {
    let reading = matches!(state.disks, Disks::Reading);
    let mut drive = column![
        row![
            heading("Drive"),
            space::horizontal(),
            plain("Refresh", (!reading).then_some(Message::Refresh), colors),
        ]
        .align_y(Center)
    ]
    .spacing(8);
    match &state.disks {
        Disks::Reading => drive = drive.push(note("Looking for sticks and USB disks.", colors.dim)),
        Disks::Failed(why) => drive = drive.push(note(why.clone(), colors.error)),
        Disks::Read(listed) if listed.is_empty() => {
            drive = drive.push(note("No stick or USB disk is plugged in.", colors.dim));
        }
        Disks::Read(listed) => {
            let selected = listed
                .iter()
                .position(|item| state.selected.as_deref() == Some(item.disk.path.as_str()));
            for (index, item) in listed.iter().enumerate() {
                if index > 0 {
                    drive = drive.push(line(colors));
                }
                drive = drive.push(disk(index, item, selected, colors));
            }
        }
    }
    drive.into()
}

/// A disk as eclipse-flash lists it: what it is, and why it is refused when it is. One that passes can
/// be selected anywhere on its row.
fn disk(
    index: usize,
    item: &Listed,
    selected: Option<usize>,
    colors: Colors,
) -> Element<'_, Message> {
    let color = if item.refused.is_some() {
        colors.dim
    } else {
        colors.text
    };
    let mut about = column![].spacing(2).width(Fill);
    for said in item.disk.describe() {
        about = about.push(text(said).size(TEXT_SIZE).color(color));
    }
    if let Some(why) = &item.refused {
        about = about.push(text(why.clone()).size(TEXT_SIZE));
    }
    let choice: Element<'_, Message> = if item.refused.is_some() {
        space().width(RADIO_SIZE).into()
    } else {
        radio("", index, selected, Message::Select)
            .size(RADIO_SIZE)
            .spacing(0)
            .style(move |_, status| radio_style(colors, status))
            .into()
    };
    let row = row![choice, about].spacing(10).padding([6, 0]);
    if item.refused.is_some() {
        row.into()
    } else {
        mouse_area(row).on_press(Message::Select(index)).into()
    }
}

/// Linux only: the passphrase for persist now, or on the drive's own screen when it first starts.
fn persist(state: &Flash, colors: Colors) -> Element<'_, Message> {
    let choice = Some(state.later);
    let option = |label: &'static str, later: bool| {
        radio(label, later, choice, Message::Later)
            .size(RADIO_SIZE)
            .text_size(TEXT_SIZE)
            .spacing(10)
            .style(move |_, status| radio_style(colors, status))
    };
    let mut persist = column![
        heading("Persist"),
        note(
            "Persist holds your files and settings on the drive, encrypted with a passphrase.",
            colors.dim
        ),
        option("Choose the passphrase now", false),
        option("Choose it on the drive when it first starts", true),
    ]
    .spacing(8);
    if !state.later {
        persist = persist
            .push(field(
                "Passphrase",
                &state.passphrase,
                Message::Passphrase,
                true,
                colors,
            ))
            .push(field(
                "Type it again",
                &state.again,
                Message::Again,
                true,
                colors,
            ));
        if let Some(problem) = state.passphrase_problem() {
            persist = persist.push(note(problem, colors.error));
        }
    }
    persist.into()
}

fn confirm(state: &Flash, colors: Colors) -> Element<'_, Message> {
    let mut confirm = column![heading("Erase and write")].spacing(8);
    match (&state.program, state.disk()) {
        (Err(why), _) => confirm = confirm.push(note(why.clone(), colors.error)),
        (Ok(_), None) => {
            confirm = confirm.push(note("Select a stick or USB disk above.", colors.dim));
        }
        (Ok(_), Some(item)) => {
            let what = if item.disk.serial.is_some() {
                "serial"
            } else {
                "name"
            };
            confirm = confirm
                .push(
                    text(format!(
                        "Everything on {} will be erased. To go on, type its {what}, {}.",
                        item.disk.path,
                        item.disk.confirmation()
                    ))
                    .size(TEXT_SIZE),
                )
                .push(field(
                    if item.disk.serial.is_some() {
                        "Serial"
                    } else {
                        "Name"
                    },
                    &state.typed,
                    Message::Type,
                    false,
                    colors,
                ));
        }
    }
    confirm
        .push(row![
            space::horizontal(),
            primary(
                "Write",
                state.job().is_some().then_some(Message::Write),
                colors
            )
        ])
        .into()
}

/// The page that shows what eclipse-flash prints while it writes, and the sentence it ends on.
fn steps(writing: &Writing, colors: Colors) -> Element<'_, Message> {
    let title = match writing.ended {
        None => format!("Writing Eclipse onto {}", writing.disk),
        Some(true) => format!("Eclipse was written onto {}", writing.disk),
        Some(false) => format!("Eclipse was not written onto {}", writing.disk),
    };
    let mut lines = column![].spacing(4);
    for said in &writing.lines {
        lines = lines.push(match said {
            Line::Said(said) => text(said).size(TEXT_SIZE),
            Line::Failed(why) => text(why).size(TEXT_SIZE).color(colors.error),
        });
    }
    let bottom: Element<'_, Message> = match writing.ended {
        None => note(
            if writing.closing {
                "The window stays open until the drive is written. Keep the drive plugged in."
            } else {
                "Keep the drive plugged in until this is done."
            },
            colors.dim,
        ),
        Some(_) => row![
            space::horizontal(),
            primary("Done", Some(Message::Done), colors)
        ]
        .into(),
    };
    container(
        column![
            text(title).size(TITLE_SIZE).font(BOLD),
            scrollable(lines.width(Fill)).height(Fill).anchor_bottom(),
            bottom,
        ]
        .spacing(16)
        .padding(24)
        .max_width(640),
    )
    .center_x(Fill)
    .into()
}

fn heading(label: &str) -> Element<'_, Message> {
    text(label).size(TEXT_SIZE).font(BOLD).into()
}

fn note<'a>(said: impl text::IntoFragment<'a>, color: Color) -> Element<'a, Message> {
    text(said).size(TEXT_SIZE).color(color).into()
}

fn line<'a>(colors: Colors) -> Element<'a, Message> {
    rule::horizontal(1)
        .style(move |_| rule::Style {
            color: colors.line,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

fn field<'a>(
    placeholder: &str,
    value: &str,
    on_input: fn(String) -> Message,
    secure: bool,
    colors: Colors,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(on_input)
        .secure(secure)
        .size(TEXT_SIZE)
        .padding([6, 8])
        .style(move |_, status| {
            let edge = match status {
                text_input::Status::Focused { .. } => colors.accent,
                _ => colors.edge,
            };
            text_input::Style {
                background: colors.field.into(),
                border: Border {
                    color: edge,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                icon: colors.dim,
                placeholder: colors.dim,
                value: colors.text,
                selection: Color {
                    a: 0.3,
                    ..colors.accent
                },
            }
        })
        .into()
}

/// The default button: the one blue thing on the page.
fn primary(label: &str, on_press: Option<Message>, colors: Colors) -> Element<'_, Message> {
    button(text(label).size(TEXT_SIZE))
        .padding([6, 20])
        .on_press_maybe(on_press)
        .style(move |_, status| {
            let fill = match status {
                button::Status::Disabled => Color {
                    a: 0.45,
                    ..colors.accent
                },
                button::Status::Hovered | button::Status::Pressed => colors.accent.scale_alpha(0.9),
                button::Status::Active => colors.accent,
            };
            button::Style {
                background: Some(fill.into()),
                text_color: colors.on_accent,
                border: Border {
                    radius: 4.0.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        })
        .into()
}

fn plain(label: &str, on_press: Option<Message>, colors: Colors) -> Element<'_, Message> {
    button(text(label).size(TEXT_SIZE))
        .padding([6, 14])
        .on_press_maybe(on_press)
        .style(move |_, status| {
            let text_color = match status {
                button::Status::Disabled => colors.dim,
                _ => colors.text,
            };
            let fill = match status {
                button::Status::Hovered | button::Status::Pressed => colors.line,
                _ => colors.button,
            };
            button::Style {
                background: Some(fill.into()),
                text_color,
                border: Border {
                    color: colors.edge,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..button::Style::default()
            }
        })
        .into()
}

fn radio_style(colors: Colors, status: radio::Status) -> radio::Style {
    let (radio::Status::Active { is_selected } | radio::Status::Hovered { is_selected }) = status;
    radio::Style {
        background: colors.field.into(),
        dot_color: colors.accent,
        border_width: 1.0,
        border_color: if is_selected {
            colors.accent
        } else {
            colors.edge
        },
        text_color: Some(colors.text),
    }
}
