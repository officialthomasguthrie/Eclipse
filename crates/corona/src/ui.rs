//! The panel: a strip along the top of the screen with the field in it. Drawn with iced on
//! the software renderer, placed by the layer-shell protocol, so it works on any machine the
//! drive meets. Sizes and colours are the ones the boot test counts.

use iced::widget::{container, row, text, text_input};
use iced::{
    Border, Color, Element, Font, Length, Subscription, Task, Theme, font, keyboard, theme,
};
use iced_layershell::actions::LayerShellCustomActionWithId;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};

use crate::launcher::{self, App};
use crate::os::{self, Action};
use crate::route::{self, Interpretation};

/// Height of the panel in logical pixels. The compositor keeps windows below it.
pub const PANEL_HEIGHT: u32 = 32;
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

struct Corona {
    input: String,
    apps: Vec<App>,
    notice: Option<Notice>,
    pending: Option<Action>,
}

struct Notice {
    text: String,
    error: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Input(String),
    Submit,
    Escape,
    Done(Result<String, String>),
}

// the layer-shell runtime asks every message whether it is one of its own actions. none is.
impl TryFrom<Message> for LayerShellCustomActionWithId {
    type Error = Message;

    fn try_from(message: Message) -> Result<Self, Message> {
        Err(message)
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
        notice: None,
        pending: None,
    };
    (state, iced::widget::operation::focus(FIELD_ID))
}

fn subscription(_: &Corona) -> Subscription<Message> {
    keyboard::listen().filter_map(|event| match event {
        keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            ..
        } => Some(Message::Escape),
        _ => None,
    })
}

fn update(state: &mut Corona, message: Message) -> Task<Message> {
    match message {
        Message::Input(value) => {
            if state.pending.take().is_some() {
                state.notice = None;
            }
            state.input = value;
            Task::none()
        }
        Message::Submit => submit(state),
        Message::Escape => {
            state.input.clear();
            state.pending = None;
            state.notice = None;
            Task::none()
        }
        Message::Done(result) => {
            state.notice = Some(match result {
                Ok(output) => Notice {
                    text: first_line(&output, "Done"),
                    error: false,
                },
                Err(why) => Notice {
                    text: why,
                    error: true,
                },
            });
            Task::none()
        }
    }
}

fn submit(state: &mut Corona) -> Task<Message> {
    if let Some(action) = state.pending.take() {
        state.input.clear();
        state.notice = None;
        return start(action);
    }
    let reading = route::route(&state.input, &state.apps);
    eprintln!("corona: {:?} -> {reading:?}", state.input);
    match reading {
        Interpretation::Nothing => {}
        Interpretation::Launch(app) => {
            state.notice = Some(match launcher::launch(&app) {
                Ok(()) => Notice {
                    text: format!("Starting {}", app.name),
                    error: false,
                },
                Err(why) => Notice {
                    text: why,
                    error: true,
                },
            });
            state.input.clear();
        }
        Interpretation::Os(action) if action.mutating => {
            state.notice = Some(Notice {
                text: format!(
                    "{}? Press Enter to confirm or Escape to cancel.",
                    action.summary
                ),
                error: false,
            });
            state.pending = Some(action);
        }
        Interpretation::Os(action) => {
            state.input.clear();
            return start(action);
        }
        Interpretation::Usage(usage) => {
            state.notice = Some(Notice {
                text: usage.to_string(),
                error: true,
            });
        }
        Interpretation::Shell(_) => {
            state.notice = Some(Notice {
                text: "Nushell is not in this build yet.".into(),
                error: true,
            });
        }
        Interpretation::Ask(_) => {
            state.notice = Some(Notice {
                text: "Aura is not in this build yet.".into(),
                error: true,
            });
        }
    }
    Task::none()
}

fn start(action: Action) -> Task<Message> {
    eprintln!(
        "corona: running {} {}",
        action.program,
        action.args.join(" ")
    );
    Task::perform(async move { os::run(&action) }, Message::Done)
}

fn first_line(output: &str, if_empty: &str) -> String {
    let line = output.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        if_empty.to_string()
    } else {
        line.to_string()
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
        let color = if notice.error { ERROR } else { DIM };
        line = line.push(text(&notice.text).size(TEXT_SIZE).color(color));
    }
    container(line)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([0, 12])
        .align_y(iced::Center)
        .style(|_| container::Style {
            background: Some(PANEL.into()),
            text_color: Some(TEXT),
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
