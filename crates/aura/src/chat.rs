//! A question to the model and the answer out of its reply. The request goes to llama-server's
//! chat completions endpoint, the same one anything else on the machine can use. What the model
//! says is either an answer in words or the words of one of Corona's OS commands, which it marks
//! with an `ACTION:` first line.

use std::time::Duration;

use serde_json::{Value, json};

use crate::http;

/// What Aura tells the model before every question. The commands are the grammar Corona's field
/// reads, and Corona runs nothing from a reply that does not read as one of them.
pub const INSTRUCTIONS: &str = "You are Aura, the assistant in Eclipse OS, a personal operating \
system that runs from a USB drive. Answer in one to three plain, short sentences. Do not greet, \
do not apologize, and do not use markdown.\n\
When the person wants the computer to do something that one of these commands does, reply with \
one line only: ACTION: and the command, for example ACTION: wifi off. These are the only \
commands:\n\
wifi list, wifi status, wifi on, wifi off, wifi connect <network> [password]\n\
display brightness, display brightness <0 to 100>, display brightness +10, display brightness \
-10, display outputs\n\
volume, volume <0 to 100>, volume up, volume down, volume mute, volume unmute\n\
power off, power reboot, power suspend\n\
For anything else, answer in words and do not write ACTION.";

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

/// Asks the model on `127.0.0.1:<port>` and returns what it said.
///
/// # Errors
///
/// A sentence that says why there is no answer.
pub fn ask(port: u16, question: &str) -> Result<Reply, String> {
    let body = request(question, MAX_TOKENS);
    let response = http::send(
        port,
        "POST",
        "/v1/chat/completions",
        Some(&body),
        ANSWER_TIMEOUT,
    )
    .map_err(|e| format!("Could not reach the model: {e}."))?;
    answer(response.status, &response.body).and_then(|text| reply(&text))
}

/// The request body for one question.
pub fn request(question: &str, max_tokens: u32) -> String {
    json!({
        "messages": [
            { "role": "system", "content": INSTRUCTIONS },
            { "role": "user", "content": question },
        ],
        "max_tokens": max_tokens,
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
    fn the_instructions_name_the_mark_and_every_command() {
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
            assert!(INSTRUCTIONS.contains(words), "{words}");
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
