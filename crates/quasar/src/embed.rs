//! Vectors for search by meaning. Texts go to the embedding model's llama-server on its own socket,
//! and one vector comes back for each text, in the order the texts went.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::http;

/// How long the vectors for one call may take. A call has at most 32 parts of a file, which take
/// seconds on a slow CPU.
const EMBED_TIMEOUT: Duration = Duration::from_secs(120);

/// Asks the model behind `socket` for a vector for each text.
///
/// # Errors
///
/// A sentence that says why there are no vectors.
pub fn vectors(socket: &Path, texts: &[String]) -> Result<Vec<Vec<f64>>, String> {
    let body = json!({ "input": texts }).to_string();
    let response = http::send(socket, "POST", "/v1/embeddings", Some(&body), EMBED_TIMEOUT)
        .map_err(|e| format!("Could not reach the embedding model: {e}."))?;
    read(response.status, &response.body, texts.len())
}

/// The vectors in a reply, in the order of the texts, or why there are none.
///
/// # Errors
///
/// When the reply is an error, is not JSON, or does not have one vector of the same length for
/// each of the `count` texts.
pub fn read(status: u16, body: &[u8], count: usize) -> Result<Vec<Vec<f64>>, String> {
    let reply: Value = serde_json::from_slice(body).map_err(|_| {
        format!("The embedding model sent a reply that is not JSON (HTTP {status}).")
    })?;
    if status != 200 {
        let message = reply
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(format!(
            "The embedding model could not read the text (HTTP {status}): {message}"
        ));
    }
    let data = reply
        .get("data")
        .and_then(Value::as_array)
        .ok_or("The embedding model's reply has no vectors.")?;
    if data.len() != count {
        return Err(format!(
            "The embedding model sent {} vectors for {count} texts.",
            data.len()
        ));
    }
    let unreadable = || "The embedding model sent a vector this program cannot read.".to_string();
    let mut vectors = vec![Vec::new(); count];
    for item in data {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < count)
            .ok_or_else(unreadable)?;
        let vector = item
            .get("embedding")
            .and_then(Value::as_array)
            .and_then(|numbers| {
                numbers
                    .iter()
                    .map(Value::as_f64)
                    .collect::<Option<Vec<_>>>()
            })
            .filter(|vector| !vector.is_empty())
            .ok_or_else(unreadable)?;
        vectors[index] = vector;
    }
    let length = vectors[0].len();
    if vectors.iter().any(|vector| vector.len() != length) {
        return Err(unreadable());
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_come_back_in_the_order_of_the_texts() {
        let body = br#"{"model":"nomic","object":"list","data":[
            {"object":"embedding","index":1,"embedding":[0.0,1.0]},
            {"object":"embedding","index":0,"embedding":[1.0,0.0]}
        ],"usage":{"prompt_tokens":9,"total_tokens":9}}"#;
        assert_eq!(
            read(200, body, 2).unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
    }

    #[test]
    fn an_error_says_what_the_server_said() {
        let body = br#"{"error":{"code":500,"message":"input (3006 tokens) is too large to process","type":"server_error"}}"#;
        let why = read(500, body, 1).unwrap_err();
        assert!(why.contains("HTTP 500"), "{why}");
        assert!(why.contains("too large"), "{why}");
        assert!(read(200, b"<html>", 1).unwrap_err().contains("not JSON"));
    }

    #[test]
    fn a_vector_missing_or_unreadable_is_an_error() {
        let one = br#"{"data":[{"index":0,"embedding":[1.0]}]}"#;
        assert!(
            read(200, one, 2)
                .unwrap_err()
                .contains("1 vectors for 2 texts")
        );
        for body in [
            &br#"{"data":[{"index":0,"embedding":[1.0]},{"index":0,"embedding":[1.0]}]}"#[..],
            br#"{"data":[{"index":0,"embedding":[1.0]},{"index":2,"embedding":[1.0]}]}"#,
            br#"{"data":[{"index":0,"embedding":[1.0]},{"index":1,"embedding":["a"]}]}"#,
            br#"{"data":[{"index":0,"embedding":[1.0]},{"index":1,"embedding":[1.0,2.0]}]}"#,
        ] {
            assert!(
                read(200, body, 2).is_err(),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }
}
