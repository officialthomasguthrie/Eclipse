//! A question to the model and the answer out of its reply. The request goes to llama-server's
//! chat completions endpoint, the same one anything else on the machine reaches through the local
//! api. What the model
//! says is either an answer in words or the words of one of Corona's OS commands, which it marks
//! with an `ACTION:` first line.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::http;

/// What Aura tells the model before every question.
pub const INSTRUCTIONS: &str = "You are Aura, the assistant in Eclipse OS, a personal operating \
system that runs from a USB drive. Answer in one to three plain, short sentences. Do not greet, \
do not apologize, and do not use markdown.";

/// What Aura adds when the question is about one of the settings Corona's OS commands change.
/// The commands are the grammar Corona's field reads, and Corona runs nothing from a reply that
/// does not read as one of them.
pub const COMMANDS: &str = "When the person wants the computer to do something that one of \
these commands does, reply with one line only: ACTION: and the command, for example ACTION: wifi \
off. These are the only commands:\n\
wifi list, wifi status, wifi on, wifi off, wifi connect <network> [password]\n\
display brightness, display brightness <0 to 100>, display brightness +10, display brightness \
-10, display outputs\n\
volume, volume <0 to 100>, volume up, volume down, volume mute, volume unmute\n\
power off, power reboot, power suspend\n\
For anything else, answer in words and do not write ACTION.";

/// Words that put a question near wifi, the display, the sound or the power. A small model that
/// is offered the commands with every question proposes one for questions that have nothing to
/// do with them ("What is the capital of France?" came back as `display brightness +10`), so
/// the commands only go with a question that has one of these words in it.
const SETTING_WORDS: &[&str] = &[
    "wifi",
    "wi-fi",
    "wireless",
    "network",
    "networks",
    "internet",
    "online",
    "offline",
    "hotspot",
    "display",
    "displays",
    "screen",
    "screens",
    "monitor",
    "monitors",
    "brightness",
    "bright",
    "brighter",
    "dim",
    "dimmer",
    "darker",
    "outputs",
    "volume",
    "sound",
    "audio",
    "loud",
    "louder",
    "quiet",
    "quieter",
    "mute",
    "unmute",
    "speaker",
    "speakers",
    "power",
    "restart",
    "reboot",
    "shutdown",
    "shut",
    "sleep",
    "suspend",
];

/// Earlier turns of the chat that go before a question about a setting: two questions answered
/// in words and three commands. On a bench with the test model they kept every question about a
/// setting in words and turned every request into the right command; without them it wrote
/// `volume +10` and answered some of those questions with a command.
const EXAMPLES: &[(&str, &str)] = &[
    (
        "Is wifi slower than a cable?",
        "Usually yes. A cable is faster and steadier than wifi.",
    ),
    ("Turn the sound down", "ACTION: volume down"),
    (
        "Why does my screen flicker?",
        "A loose cable or a low refresh rate can make a screen flicker.",
    ),
    ("Put the computer to sleep", "ACTION: power suspend"),
    ("Set the brightness to 70", "ACTION: display brightness 70"),
];

/// Sampling temperature. Low, so the same question gets the same kind of reply.
const TEMPERATURE: f64 = 0.2;

/// How the first line of a reply that proposes a command starts.
const ACTION: &str = "ACTION:";

/// The longest answer, in tokens.
const MAX_TOKENS: u32 = 512;

/// How long an answer may take. A small model on a slow CPU needs tens of seconds.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// What the model said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Plain text for the person.
    Answer(String),
    /// The words of an OS command, such as `volume 40`. Corona reads them with its own parser.
    Action(String),
}

impl Reply {
    /// `answer` or `action`, the first string `Ask` returns.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Answer(_) => "answer",
            Self::Action(_) => "action",
        }
    }

    /// The answer or the command's words, the second string.
    pub fn into_text(self) -> String {
        match self {
            Self::Answer(text) | Self::Action(text) => text,
        }
    }
}

/// Asks the model behind `socket` and returns what it said.
///
/// # Errors
///
/// A sentence that says why there is no answer.
pub fn ask(socket: &Path, question: &str) -> Result<Reply, String> {
    let body = request(question, MAX_TOKENS);
    let response = http::send(
        socket,
        "POST",
        "/v1/chat/completions",
        Some(&body),
        ANSWER_TIMEOUT,
    )
    .map_err(|e| format!("Could not reach the model: {e}."))?;
    answer(response.status, &response.body).and_then(|text| reply(&text))
}

/// True when the question has a word in it about one of the settings the commands change.
pub fn about_settings(question: &str) -> bool {
    question
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .any(|word| SETTING_WORDS.contains(&word))
}

/// The instructions for one question: the commands go with it only when it is about a setting.
pub fn instructions(question: &str) -> String {
    if about_settings(question) {
        format!("{INSTRUCTIONS}\n{COMMANDS}")
    } else {
        INSTRUCTIONS.to_string()
    }
}

/// The request body for one question.
pub fn request(question: &str, max_tokens: u32) -> String {
    let mut messages = vec![json!({ "role": "system", "content": instructions(question) })];
    if about_settings(question) {
        for (asked, said) in EXAMPLES {
            messages.push(json!({ "role": "user", "content": asked }));
            messages.push(json!({ "role": "assistant", "content": said }));
        }
    }
    messages.push(json!({ "role": "user", "content": question }));
    json!({
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": TEMPERATURE,
        // qwen3 thinks out loud before it answers unless it is told not to, and the thinking
        // takes the whole token budget on a small model
        "chat_template_kwargs": { "enable_thinking": false },
    })
    .to_string()
}

/// The answer in a reply, or why there is none.
///
/// # Errors
///
/// When the reply is an error, is not JSON, or holds no text.
pub fn answer(status: u16, body: &[u8]) -> Result<String, String> {
    let reply: Value = serde_json::from_slice(body)
        .map_err(|_| format!("The model sent a reply that is not JSON (HTTP {status})."))?;
    if status != 200 {
        let message = reply
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(format!(
            "The model could not answer: {}.",
            message.trim_end_matches('.')
        ));
    }
    let content = reply
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| "The reply from the model has no answer in it.".to_string())?;
    let text = without_thinking(content).trim();
    if text.is_empty() {
        return Err("The model gave an empty answer.".into());
    }
    Ok(text.to_string())
}

/// Reads the model's text. A first line `ACTION: <words>` makes it a command, and whatever
/// follows that line is dropped; anything else is an answer. A small model does not always keep
/// to the format, so the mark is read without regard to case and without the quotes, backticks
/// and bold it may wrap around the line.
///
/// # Errors
///
/// When the first line marks a command but names none and nothing follows it.
pub fn reply(text: &str) -> Result<Reply, String> {
    let text = text.trim();
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let line = bare(first);
    let marked = line
        .get(..ACTION.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(ACTION));
    if !marked {
        return Ok(Reply::Answer(text.to_string()));
    }
    let words = bare(bare(&line[ACTION.len()..]).trim_end_matches('.'));
    if !words.is_empty() {
        return Ok(Reply::Action(words.to_string()));
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return Err("The model proposed a command without saying which.".into());
    }
    Ok(Reply::Answer(rest.to_string()))
}

/// A line without the whitespace, quotes, backticks and bold marks around it.
fn bare(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || matches!(c, '`' | '*' | '"' | '\''))
}

/// The text after a leading `<think>` block, for a server that leaves the block in.
fn without_thinking(text: &str) -> &str {
    text.trim_start()
        .strip_prefix("<think>")
        .and_then(|rest| rest.split_once("</think>"))
        .map_or(text, |(_, after)| after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_carries_the_question_and_turns_thinking_off() {
        let question = "What is \"nix\"?\nIn one line.";
        let body: Value = serde_json::from_str(&request(question, 64)).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], INSTRUCTIONS);
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], question);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn a_plain_question_is_not_offered_the_commands() {
        for question in [
            "What is the capital of France?",
            "Who wrote Hamlet?",
            "What is 12 times 7?",
            "Why is the sky blue?",
        ] {
            assert!(!about_settings(question), "{question}");
            assert_eq!(instructions(question), INSTRUCTIONS);
            let body: Value = serde_json::from_str(&request(question, 64)).unwrap();
            assert!(!body.to_string().contains("ACTION"), "{question}");
        }
    }

    #[test]
    fn a_question_about_a_setting_is_offered_the_commands() {
        for question in [
            "turn the wifi off",
            "Is public Wi-Fi safe?",
            "dim the screen",
            "make it louder",
            "mute the sound",
            "restart the computer",
            "put it to sleep",
        ] {
            assert!(about_settings(question), "{question}");
            let body: Value = serde_json::from_str(&request(question, 64)).unwrap();
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.starts_with(INSTRUCTIONS), "{question}");
            assert!(system.ends_with(COMMANDS), "{question}");
        }
        // a word has to be the whole word
        assert!(!about_settings("Who is Dimitri?"));
        assert!(!about_settings("How does a powerline adapter work?"));
    }

    #[test]
    fn a_question_about_a_setting_comes_after_the_examples() {
        let body: Value = serde_json::from_str(&request("dim the screen", 64)).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2 + 2 * EXAMPLES.len());
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert_eq!(messages.last().unwrap()["content"], "dim the screen");
        assert_eq!(body["temperature"], TEMPERATURE);
        let plain: Value = serde_json::from_str(&request("Who wrote Hamlet?", 64)).unwrap();
        assert_eq!(plain["messages"].as_array().unwrap().len(), 2);
        assert_eq!(plain["temperature"], TEMPERATURE);
    }

    #[test]
    fn the_examples_read_the_way_the_model_should_write() {
        let mut commands = 0;
        for (asked, said) in EXAMPLES {
            assert!(about_settings(asked), "{asked}");
            match reply(said) {
                Ok(Reply::Action(words)) => {
                    commands += 1;
                    let first = words.split(' ').next().unwrap();
                    assert!(COMMANDS.contains(&format!("{first} ")), "{words}");
                }
                Ok(Reply::Answer(_)) => assert!(!said.contains("ACTION"), "{said}"),
                Err(why) => panic!("{why}"),
            }
        }
        assert!(commands > 0 && commands < EXAMPLES.len());
    }

    #[test]
    fn the_commands_name_the_mark_and_every_command() {
        for words in [
            "ACTION: wifi off",
            "wifi connect <network> [password]",
            "display brightness <0 to 100>",
            "display outputs",
            "volume <0 to 100>",
            "volume unmute",
            "power off",
            "power suspend",
        ] {
            assert!(COMMANDS.contains(words), "{words}");
        }
    }

    #[test]
    fn the_answer_comes_out_of_a_real_reply() {
        // llama-server 0.4.0 with qwen3 0.6b, thinking off
        let body = br#"{"choices":[{"finish_reason":"stop","index":0,"message":{"role":"assistant","content":"The capital of France is Paris."}}],"created":1789028454,"model":"qwen3-0.6b-q8_0","system_fingerprint":"b10809-5266f24","object":"chat.completion","usage":{"completion_tokens":8,"prompt_tokens":30,"total_tokens":38},"id":"chatcmpl-FwZD1xhwzGinhos2Af50WPSqsq5gcu5M"}"#;
        assert_eq!(
            answer(200, body),
            Ok("The capital of France is Paris.".to_string())
        );
    }

    #[test]
    fn a_reply_that_only_thought_has_no_answer() {
        // the same server with thinking on and too few tokens to finish
        let body = br#"{"choices":[{"finish_reason":"length","index":0,"message":{"role":"assistant","content":"","reasoning_content":"Okay, so the user is asking"}}]}"#;
        assert_eq!(
            answer(200, body),
            Err("The model gave an empty answer.".to_string())
        );
    }

    #[test]
    fn a_think_block_left_in_the_text_is_dropped() {
        let body = br#"{"choices":[{"message":{"content":"<think>\nhmm\n</think>\n\nParis."}}]}"#;
        assert_eq!(answer(200, body), Ok("Paris.".to_string()));
        assert_eq!(
            without_thinking("<think> never closed"),
            "<think> never closed"
        );
    }

    #[test]
    fn errors_say_what_the_server_said() {
        let body =
            br#"{"error":{"code":503,"message":"Loading model","type":"unavailable_error"}}"#;
        assert_eq!(
            answer(503, body),
            Err("The model could not answer: Loading model.".to_string())
        );
        assert!(answer(200, b"<html>").is_err());
        assert!(answer(200, br#"{"choices":[]}"#).is_err());
    }

    #[test]
    fn plain_text_is_an_answer() {
        assert_eq!(
            reply("The capital of France is Paris."),
            Ok(Reply::Answer("The capital of France is Paris.".into()))
        );
        assert_eq!(
            reply("  Two lines.\nBoth kept.\n"),
            Ok(Reply::Answer("Two lines.\nBoth kept.".into()))
        );
    }

    #[test]
    fn an_action_line_is_a_command() {
        assert_eq!(
            reply("ACTION: volume 40"),
            Ok(Reply::Action("volume 40".into()))
        );
        assert_eq!(
            reply("ACTION: wifi off\nThat turns the radio off."),
            Ok(Reply::Action("wifi off".into()))
        );
    }

    #[test]
    fn an_action_line_is_read_the_way_a_small_model_writes_it() {
        for text in [
            "action: power suspend",
            "ACTION:power suspend",
            "ACTION: power suspend.",
            "`ACTION: power suspend`",
            "**ACTION:** power suspend",
            "ACTION: \"power suspend\"",
            "\n  ACTION: power suspend  \n",
        ] {
            assert_eq!(
                reply(text),
                Ok(Reply::Action("power suspend".into())),
                "{text:?}"
            );
        }
    }

    #[test]
    fn only_the_first_line_can_mark_a_command() {
        let text = "That turns the computer off.\nACTION: power off";
        assert_eq!(reply(text), Ok(Reply::Answer(text.into())));
        assert_eq!(
            reply("Actions are listed below."),
            Ok(Reply::Answer("Actions are listed below.".into()))
        );
    }

    #[test]
    fn a_mark_with_no_command_is_not_one() {
        assert!(reply("ACTION:").is_err());
        assert!(reply("ACTION: `` ").is_err());
        assert_eq!(
            reply("ACTION:\nThere is no command for that."),
            Ok(Reply::Answer("There is no command for that.".into()))
        );
    }

    #[test]
    fn a_reply_splits_into_the_two_strings_ask_returns() {
        let action = Reply::Action("wifi on".into());
        assert_eq!(action.kind(), "action");
        assert_eq!(action.into_text(), "wifi on");
        let answer = Reply::Answer("Paris.".into());
        assert_eq!(answer.kind(), "answer");
        assert_eq!(answer.into_text(), "Paris.");
    }
}
