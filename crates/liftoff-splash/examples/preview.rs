//! Draws the text boot into a png, the way the splash draws it on a screen.
//!
//! preview REGULAR.ttf BOLD.ttf CONSOLE OUT.png [--size 1280x800] [--scale 1] [--title TEXT]
//!         [--secret QUESTION --typed N] [--question QUESTION --answer TEXT] [--details] [--beside]
//!
//! CONSOLE holds what was written to the console, like plymouth's /var/log/boot.log.

use std::error::Error;

use liftoff_splash::console::Console;
use liftoff_splash::font::Fonts;
use liftoff_splash::logo::Logo;
use liftoff_splash::screen::{Banner, Frame, Prompt, View};

fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: preview REGULAR.ttf BOLD.ttf CONSOLE OUT.png [options]";
    let mut args = std::env::args().skip(1);
    let regular = args.next().ok_or(usage)?;
    let bold = args.next().ok_or(usage)?;
    let written = args.next().ok_or(usage)?;
    let out = args.next().ok_or(usage)?;
    let (mut width, mut height, mut scale) = (1280, 800, 1);
    let mut title = "Rift 0.1.0".to_string();
    let mut prompt = None;
    let mut typed = 0;
    let mut details = false;
    let mut banner = Banner::Under;
    while let Some(option) = args.next() {
        let mut value = || args.next().ok_or(format!("{option} needs a value"));
        match option.as_str() {
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                width = w.parse()?;
                height = h.parse()?;
            }
            "--scale" => scale = value()?.parse()?,
            "--title" => title = value()?,
            "--secret" => {
                prompt = Some(Prompt::Secret {
                    question: value()?,
                    typed: 0,
                });
            }
            "--typed" => typed = value()?.parse()?,
            "--question" => {
                prompt = Some(Prompt::Visible {
                    question: value()?,
                    answer: String::new(),
                });
            }
            "--answer" => {
                let text = value()?;
                if let Some(Prompt::Visible { answer, .. }) = &mut prompt {
                    *answer = text;
                }
            }
            "--details" => details = true,
            "--beside" => banner = Banner::Beside,
            other => return Err(format!("{other} is not an option. {usage}").into()),
        }
    }
    if let Some(Prompt::Secret { typed: count, .. }) = &mut prompt {
        *count = typed;
    }

    let mut console = Console::new();
    console.write(&std::fs::read(written)?);
    let mut fonts = Fonts::new(std::fs::read(regular)?, std::fs::read(bold)?, 8 * scale)?;
    let mut frame = Frame::new(width * scale, height * scale);
    let view = View {
        console: &console,
        prompt: prompt.as_ref(),
        title: &title,
        details,
        banner,
    };
    frame.draw(&mut fonts, &Logo::new(), &view);

    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut encoder = png::Encoder::new(
        file,
        u32::try_from(frame.width())?,
        u32::try_from(frame.height())?,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&frame.rgb())?;
    Ok(())
}
