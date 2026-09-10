//! A question to the model and the answer out of its reply. The request goes to llama-server's
//! chat completions endpoint, the same one anything else on the machine can use.

use std::time::Duration;

use serde_json::{Value, json};

use crate::http;

/// What Aura tells the model before every question.
pub const INSTRUCTIONS: &str = "You are Aura, the assistant in Eclipse OS, a personal operating \
system that runs from a USB drive. Answer in plain, short sentences. Do not greet, do not \
apologize, and do not use markdown.";

/// The longest answer, in tokens.
const MAX_TOKENS: u32 = 512;

/// How long an answer may take. A small model on a slow CPU needs tens of seconds.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// Asks the model on `127.0.0.1:<port>` and returns its answer.
///
/// # Errors
///
/// A sentence that says why there is no answer.
pub fn ask(port: u16, question: &str) -> Result<String, String> {
    let body = request(question, MAX_TOKENS);
    let reply = http::send(
        port,
        "POST",
        "/v1/chat/completions",
        Some(&body),
        ANSWER_TIMEOUT,
    )
    .map_err(|e| format!("Could not reach the model: {e}."))?;
    answer(reply.status, &reply.body)
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
}
