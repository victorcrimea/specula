//! Every difference between the attributes the server sent the engine and
//! the ones the engine reports it launched with (M21).

use serde_json::Value;

const MAX_LINES: usize = 40;
const MAX_BYTES: usize = 200;

/// One line per differing value, nested objects and arrays included.
pub fn differences(sent: &Value, launched: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    walk("", sent, launched, &mut lines);
    if lines.len() > MAX_LINES {
        let more = lines.len() - (MAX_LINES - 1);
        lines.truncate(MAX_LINES - 1);
        lines.push(format!("and {more} more"));
    }
    lines
}

fn walk(path: &str, sent: &Value, launched: &Value, lines: &mut Vec<String>) {
    match (sent, launched) {
        (Value::Object(left), Value::Object(right)) => {
            let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => walk(&child, left, right, lines),
                    (left, right) => lines.push(line(&child, left, right)),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for index in 0..left.len().max(right.len()) {
                let child = format!("{path}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(left), Some(right)) => walk(&child, left, right, lines),
                    (left, right) => lines.push(line(&child, left, right)),
                }
            }
        }
        // `JSON.stringify` writes whole floats as integers, while the
        // server's `serde_json` keeps `1` and `1.0` distinct.
        (Value::Number(left), Value::Number(right)) => {
            if left.as_f64() != right.as_f64() {
                lines.push(line(path, Some(sent), Some(launched)));
            }
        }
        _ => {
            if sent != launched {
                lines.push(line(path, Some(sent), Some(launched)));
            }
        }
    }
}

fn line(path: &str, sent: Option<&Value>, launched: Option<&Value>) -> String {
    format!(
        "{path}: sent {}, engine {}",
        display(sent),
        display(launched)
    )
}

fn display(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "absent".to_string();
    };
    let text = value.to_string();
    if text.len() <= MAX_BYTES {
        return text;
    }
    let mut end = MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}
