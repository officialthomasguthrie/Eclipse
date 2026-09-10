//! The panel: a strip along the top of the screen with the field in it, and under the field a
//! list of results and a line for what went wrong. Drawn with iced on the software renderer,
//! placed by the layer-shell protocol, so it works on any machine the drive meets. Sizes and
//! colours are the ones the boot test counts.

use iced::widget::{column, container, row, text, text_input};
use iced::{
    Border, Color, Element, Font, Length, Subscription, Task, Theme, event, font, keyboard, theme,
};
use iced_layershell::actions::{LayerShellCustomAction, LayerShellCustomActionWithId};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};

use crate::aura;
use crate::control::{self, Command};
use crate::launcher::{self, App};
use crate::nu;
use crate::os::{self, Action};
use crate::route::{self, Interpretation};

/// Height of the field's row in logical pixels. The compositor keeps windows below it, and it
/// stays the exclusive zone however tall the panel grows, so the desktop never jumps.
pub const PANEL_HEIGHT: u32 = 32;
/// Height of one row of the result list.
const ROW_HEIGHT: u32 = 22;
/// Height of the error line.
const ERROR_HEIGHT: u32 = 22;
/// The gap under whatever the panel grew to show.
const BOTTOM_PAD: u32 = 4;
/// How many results the list shows at once.
const ROWS: usize = 8;
const FIELD_WIDTH: f32 = 480.0;
const TEXT_SIZE: f32 = 14.0;
const FIELD_ID: &str = "field";

const PANEL: Color = Color::from_rgb(0.118, 0.118, 0.118); // #1e1e1e
const FIELD: Color = Color::from_rgb(0.180, 0.180, 0.180); // #2e2e2e
const EDGE: Color = Color::from_rgb(0.235, 0.235, 0.235); // #3c3c3c
const TEXT: Color = Color::from_rgb(0.902, 0.902, 0.902); // #e6e6e6
const DIM: Color = Color::from_rgb(0.549, 0.549, 0.549); // #8c8c8c
const ACCENT: Color = Color::from_rgb(0.471, 0.682, 0.929); // #78aeed
const ERROR: Color = Color::from_rgb(0.878, 0.427, 0.427); // #e06d6d
const OK: Color = Color::from_rgb(0.4, 0.7, 0.4);
const WARN: Color = Color::from_rgb(0.85, 0.7, 0.3);

const FONT: Font = Font {
    family: font::Family::Name("Noto Sans"),
    ..Font::DEFAULT
};
// what a command printed is terminal output, and a table only lines up in a fixed width
const MONO: Font = Font {
    family: font::Family::Name("DejaVu Sans Mono"),
    ..Font::DEFAULT
};

struct Corona {
    input: String,
    apps: Vec<App>,
    results: Results,
    selected: usize,
    error: Option<String>,
    notice: Option<String>,
    pending: Option<Action>,
    height: u32,
}

/// What the list under the field is showing.
enum Results {
    /// Nothing, and the panel is only the field's row high.
    None,
    /// The apps the words match, best first. One of them is selected.
    Matches(Vec<App>),
    /// What a command or a pipeline printed.
    Output(Vec<String>),
    /// Aura's answer, wrapped into rows.
    Answer(Vec<String>),
}

impl Results {
    fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Matches(apps) => apps.len(),
            Self::Output(lines) | Self::Answer(lines) => lines.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone)]
enum Message {
    Input(String),
    Submit,
    Move(isize),
    Escape,
    Done(Result<String, String>),
    Answered(Result<(String, String), String>),
    Typed(Command),
    Resize(u32),
}

// the layer-shell runtime asks every message whether it is one of its own actions. only the
// resize is: the panel grows and shrinks with the list under the field, and the surface has to
// grow with it. the exclusive zone stays at the field's own height
impl TryFrom<Message> for LayerShellCustomActionWithId {
    type Error = Message;

    fn try_from(message: Message) -> Result<Self, Message> {
        match message {
            Message::Resize(height) => Ok(Self::new(
                None,
                LayerShellCustomAction::SizeChange((0, height)),
            )),
            other => Err(other),
        }
    }
}

/// Open the panel on the session's Wayland display and run until it is closed.
///
/// # Errors
///
/// When there is no display or the compositor has no layer-shell.
pub fn run(apps: Vec<App>) -> Result<(), iced_layershell::Error> {
    iced_layershell::application(move || boot(apps.clone()), "corona", update, view)
        .theme(|_: &Corona| Theme::custom("Eclipse", PALETTE))
        .subscription(subscription)
        .settings(Settings {
            id: Some("dev.eclipse.Corona".into()),
            default_font: FONT,
            default_text_size: TEXT_SIZE.into(),
            layer_settings: LayerShellSettings {
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                layer: Layer::Top,
                exclusive_zone: i32::try_from(PANEL_HEIGHT).unwrap_or(0),
                size: Some((0, PANEL_HEIGHT)),
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                ..LayerShellSettings::default()
            },
            ..Settings::default()
        })
        .run()
}

const PALETTE: theme::Palette = theme::Palette {
    background: PANEL,
    text: TEXT,
    primary: ACCENT,
    success: OK,
    warning: WARN,
    danger: ERROR,
};

fn boot(apps: Vec<App>) -> (Corona, Task<Message>) {
    let state = Corona {
        input: String::new(),
        apps,
        results: Results::None,
        selected: 0,
        error: None,
        notice: None,
        pending: None,
        height: PANEL_HEIGHT,
    };
    (state, iced::widget::operation::focus(FIELD_ID))
}

fn subscription(_: &Corona) -> Subscription<Message> {
    Subscription::batch([keys(), terminal()])
}

// the field takes the printable keys and Escape for itself, so these come from every event, not
// only the ones no widget wanted
fn keys() -> Subscription<Message> {
    event::listen_with(|event, _, _| match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => match key {
            keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Message::Escape),
            keyboard::Key::Named(keyboard::key::Named::ArrowDown) => Some(Message::Move(1)),
            keyboard::Key::Named(keyboard::key::Named::ArrowUp) => Some(Message::Move(-1)),
            _ => None,
        },
        _ => None,
    })
}

// the socket in the runtime directory, read on a thread of its own
fn terminal() -> Subscription<Message> {
    Subscription::run(|| {
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            if let Err(why) = control::serve(|command| {
                let _ = sender.unbounded_send(Message::Typed(command));
            }) {
                eprintln!("corona: {why}");
            }
        });
        receiver
    })
}

fn update(state: &mut Corona, message: Message) -> Task<Message> {
    let task = match message {
        Message::Input(value) => {
            typed(state, value);
            Task::none()
        }
        Message::Submit => submit(state),
        Message::Move(step) => {
            step_selection(state, step);
            Task::none()
        }
        Message::Escape => {
            clear(state);
            // the field drops its own focus on Escape, so take it back
            iced::widget::operation::focus(FIELD_ID)
        }
        Message::Done(result) => {
            finish(state, result);
            Task::none()
        }
        Message::Answered(result) => answered(state, result),
        Message::Typed(command) => match command {
            Command::Type(words) => {
                typed(state, words);
                Task::none()
            }
            Command::Enter(words) => {
                if !words.is_empty() {
                    typed(state, words);
                }
                submit(state)
            }
            Command::Escape => {
                clear(state);
                Task::none()
            }
        },
        // the runtime takes this one before update ever sees it
        Message::Resize(_) => Task::none(),
    };
    Task::batch([task, resize(state)])
}

/// The panel is the field's row, plus the list, plus the error line.
fn wanted_height(state: &Corona) -> u32 {
    let rows = u32::try_from(state.results.len()).unwrap_or(0) * ROW_HEIGHT;
    let error = if state.error.is_some() {
        ERROR_HEIGHT
    } else {
        0
    };
    let under = rows + error;
    if under == 0 {
        PANEL_HEIGHT
    } else {
        PANEL_HEIGHT + under + BOTTOM_PAD
    }
}

/// Ask the compositor for a taller or shorter surface when the panel changed shape.
fn resize(state: &mut Corona) -> Task<Message> {
    let wanted = wanted_height(state);
    if wanted == state.height {
        return Task::none();
    }
    state.height = wanted;
    Task::done(Message::Resize(wanted))
}

/// New words in the field: what they match goes in the list, and anything the last line left
/// behind goes away.
fn typed(state: &mut Corona, value: String) {
    state.pending = None;
    state.notice = None;
    state.error = None;
    state.input = value;
    state.selected = 0;
    // only an app shows a list while typing. a command or a pipeline has nothing to show until
    // it has run, and a list that does not agree with what Enter does is a trap
    state.results = match route::route(&state.input, &state.apps) {
        Interpretation::Launch(_) => {
            let mut found = route::matches(&state.input, &state.apps);
            found.truncate(ROWS);
            Results::Matches(found.into_iter().cloned().collect())
        }
        _ => Results::None,
    };
}

/// Empty field, empty list, nothing pending.
fn clear(state: &mut Corona) {
    state.input.clear();
    state.results = Results::None;
    state.selected = 0;
    state.error = None;
    state.notice = None;
    state.pending = None;
}

/// Up and down walk the matches. Output rows are not a menu, nothing to select there.
fn step_selection(state: &mut Corona, step: isize) {
    let Results::Matches(apps) = &state.results else {
        return;
    };
    let last = apps.len().saturating_sub(1);
    if step > 0 {
        state.selected = if state.selected >= last {
            0
        } else {
            state.selected + 1
        };
    } else {
        state.selected = if state.selected == 0 {
            last
        } else {
            state.selected - 1
        };
    }
}

fn submit(state: &mut Corona) -> Task<Message> {
    if let Some(action) = state.pending.take() {
        state.input.clear();
        state.notice = None;
        return start(action);
    }
    // the list is a menu: Enter takes the row that is selected, not always the first
    if let Results::Matches(apps) = &state.results {
        if let Some(app) = apps.get(state.selected).cloned() {
            launch(state, &app);
            return Task::none();
        }
    }
    let reading = route::route(&state.input, &state.apps);
    eprintln!("corona: {:?} -> {reading:?}", state.input);
    match reading {
        Interpretation::Nothing => {}
        Interpretation::Launch(app) => launch(state, &app),
        Interpretation::Os(action) => return propose(state, action),
        Interpretation::Usage(usage) => {
            state.results = Results::None;
            state.error = Some(usage.to_string());
        }
        Interpretation::Shell(line) => {
            state.input.clear();
            state.results = Results::None;
            state.error = None;
            return Task::perform(async move { nu::run(&line) }, Message::Done);
        }
        Interpretation::Ask(question) => {
            state.input.clear();
            state.results = Results::None;
            state.error = None;
            state.notice = Some("Asking Aura".into());
            return ask(question);
        }
    }
    Task::none()
}

/// An OS command, typed or proposed by Aura. One that changes something waits for a second Enter,
/// the rest runs at once.
fn propose(state: &mut Corona, action: Action) -> Task<Message> {
    if action.mutating {
        state.notice = Some(format!(
            "{}? Press Enter to confirm or Escape to cancel.",
            action.summary
        ));
        state.pending = Some(action);
        return Task::none();
    }
    state.input.clear();
    state.notice = None;
    start(action)
}

/// The question goes to aurad on a thread of its own. An answer can take a minute, and the
/// executor's few threads also carry the socket the terminal types on.
fn ask(question: String) -> Task<Message> {
    Task::perform(
        async move {
            let (sender, receiver) = iced::futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let _ = sender.send(aura::ask(&question));
            });
            receiver
                .await
                .unwrap_or_else(|_| Err("Aura stopped before it answered.".into()))
        },
        Message::Answered,
    )
}

/// Aura's reply: an answer goes in the list, a command is handled like a typed one, anything
/// else goes on the error line.
fn answered(state: &mut Corona, result: Result<(String, String), String>) -> Task<Message> {
    let reply = match result {
        Ok((kind, text)) => aura::read(&kind, &text),
        Err(why) => aura::Reply::Refused(why),
    };
    eprintln!("corona: aura -> {reply:?}");
    state.notice = None;
    state.error = None;
    state.results = Results::None;
    match reply {
        aura::Reply::Answer(answer) => {
            state.results = Results::Answer(aura::rows(&answer, ROWS));
            Task::none()
        }
        aura::Reply::Action(action) => propose(state, action),
        aura::Reply::Refused(why) => {
            state.error = Some(why);
            Task::none()
        }
    }
}

fn launch(state: &mut Corona, app: &App) {
    match launcher::launch(app) {
        Ok(()) => {
            state.notice = Some(format!("Starting {}", app.name));
            state.error = None;
        }
        Err(why) => state.error = Some(why),
    }
    state.input.clear();
    state.results = Results::None;
    state.selected = 0;
}

fn start(action: Action) -> Task<Message> {
    eprintln!(
        "corona: running {} {}",
        action.program,
        action.args.join(" ")
    );
    Task::perform(async move { os::run(&action) }, Message::Done)
}

/// What a command or a pipeline printed goes in the list, what it complained about goes on the
/// error line.
fn finish(state: &mut Corona, result: Result<String, String>) {
    match result {
        Ok(output) => {
            let rows = nu::rows(&output, ROWS);
            state.error = None;
            state.results = if rows.is_empty() {
                state.notice = Some("Done".into());
                Results::None
            } else {
                state.notice = None;
                Results::Output(rows)
            };
        }
        Err(why) => {
            state.notice = None;
            state.results = Results::None;
            state.error = Some(why);
        }
    }
}

fn view(state: &Corona) -> Element<'_, Message> {
    let field = text_input("Type an app, a command or a question", &state.input)
        .id(FIELD_ID)
        .on_input(Message::Input)
        .on_submit(Message::Submit)
        .width(FIELD_WIDTH)
        .size(TEXT_SIZE)
        .padding([3, 8])
        .style(field_style);
    let mut line = row![field].spacing(12).align_y(iced::Center);
    if let Some(notice) = &state.notice {
        line = line.push(text(notice).size(TEXT_SIZE).color(DIM));
    }
    let top = container(line)
        .width(Length::Fill)
        .height(PANEL_HEIGHT)
        .padding([0, 12])
        .align_y(iced::Center);

    let mut panel = column![top];
    if !state.results.is_empty() {
        panel = panel.push(container(list(state)).padding([0, 12]));
    }
    if let Some(why) = &state.error {
        panel = panel.push(
            container(text(why).size(TEXT_SIZE).color(ERROR))
                .height(ERROR_HEIGHT)
                .padding([0, 12])
                .align_y(iced::Center),
        );
    }
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(PANEL.into()),
            text_color: Some(TEXT),
            ..container::Style::default()
        })
        .into()
}

/// The rows under the field: the apps the words match, what the last line printed, or Aura's
/// answer.
fn list(state: &Corona) -> Element<'_, Message> {
    let mut rows = column![];
    match &state.results {
        Results::None => {}
        Results::Matches(apps) => {
            for (index, app) in apps.iter().enumerate() {
                rows = rows.push(entry(&app.name, FONT, index == state.selected));
            }
        }
        Results::Output(lines) => {
            for output in lines {
                rows = rows.push(entry(output, MONO, false));
            }
        }
        Results::Answer(lines) => {
            for answer in lines {
                rows = rows.push(entry(answer, FONT, false));
            }
        }
    }
    container(rows)
        .width(FIELD_WIDTH)
        .clip(true)
        .style(|_| container::Style {
            background: Some(FIELD.into()),
            border: Border {
                color: EDGE,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn entry(label: &str, font: Font, selected: bool) -> Element<'_, Message> {
    let colour = if selected { PANEL } else { TEXT };
    let body = text(label)
        .size(TEXT_SIZE)
        .font(font)
        .color(colour)
        .wrapping(text::Wrapping::None);
    container(body)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding([0, 8])
        .align_y(iced::Center)
        .clip(true)
        .style(move |_| container::Style {
            background: selected.then(|| ACCENT.into()),
            ..container::Style::default()
        })
        .into()
}

fn field_style(_: &Theme, status: text_input::Status) -> text_input::Style {
    let edge = match status {
        text_input::Status::Focused { .. } => ACCENT,
        _ => EDGE,
    };
    text_input::Style {
        background: FIELD.into(),
        border: Border {
            color: edge,
            width: 1.0,
            radius: 4.0.into(),
        },
        icon: TEXT,
        placeholder: DIM,
        value: TEXT,
        selection: Color { a: 0.4, ..ACCENT },
    }
}
