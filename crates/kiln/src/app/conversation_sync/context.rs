//! How much of a long history a Claude projection loads into Claude Code's context: a
//! compaction boundary keeps the recent part in context, and the summary after it lists the
//! user's earlier requests.
use super::format::Message;
use serde_json::Value;

/// Bytes of message text a Claude projection keeps in context after Kiln's boundary.
pub(super) const BUDGET: usize = 320 * 1024;
/// Bytes in context after the last boundary beyond which an append starts a new boundary.
pub(super) const LIMIT: usize = 2 * BUDGET;
/// Bytes counted for one image.
const IMAGE: usize = 8 * 1024;
/// Bytes of earlier requests the summary lists, and of one listed request.
const LISTED: usize = 24 * 1024;
const REQUEST: usize = 4 * 1024;

/// Where a Claude projection starts Claude Code's context: the index of the first message
/// after the boundary, and the summary that stands for the messages before it.
pub(super) struct Compaction {
    pub at: usize,
    pub summary: String,
    /// Estimated tokens of the history before the boundary.
    pub tokens: usize,
}

pub(super) fn size(message: &Message) -> usize {
    message.text.len() + message.images.len() * IMAGE
}

/// The earliest message from which the rest of `messages` holds at most `budget` bytes and
/// that `starts` allows; the last allowed message when even that one is larger.
pub(super) fn fit(messages: &[Message], starts: impl Fn(usize) -> bool, budget: usize) -> usize {
    let mut kept = 0;
    let mut first = messages.len();
    for i in (0..messages.len()).rev() {
        kept += size(&messages[i]);
        if kept > budget && first < messages.len() {
            break;
        }
        if starts(i) {
            first = i;
        }
    }
    first.min(messages.len())
}

/// The boundary of a history written whole: none when it fits in `BUDGET`.
pub(super) fn of_history(messages: &[Message], starts: impl Fn(usize) -> bool) -> Option<Compaction> {
    if messages.iter().map(size).sum::<usize>() <= BUDGET {
        return None;
    }
    let at = fit(messages, starts, BUDGET);
    (at > 0 && at < messages.len()).then(|| compaction(messages[..at].iter(), at))
}

/// The compaction before message `at` of the messages being written, given every message
/// before it.
pub(super) fn compaction<'a>(earlier: impl Iterator<Item = &'a Message> + Clone, at: usize) -> Compaction {
    Compaction { at, summary: summary(earlier.clone()), tokens: earlier.map(size).sum::<usize>() / 4 }
}

/// The text standing for `earlier`: what the boundary means and the user's requests in it,
/// oldest first, the newest ones when they do not all fit.
pub(super) fn summary<'a>(earlier: impl IntoIterator<Item = &'a Message>) -> String {
    let requests: Vec<&str> = earlier
        .into_iter()
        .filter(|m| m.role == "user")
        .map(|m| m.text.trim())
        .filter(|t| !t.is_empty() && !t.starts_with("<local-command-") && !t.starts_with("<command-name>"))
        .collect();
    let mut listed = Vec::new();
    let mut total = 0;
    for request in requests.iter().rev() {
        let shown = shorten(request);
        total += shown.len();
        if total > LISTED && !listed.is_empty() {
            break;
        }
        listed.push(shown);
    }
    listed.reverse();
    let mut text = String::from(
        "This session continues a conversation whose earlier part is not loaded into context. \
         Kiln synchronized the conversation from another coding agent and keeps its recent part \
         in context; the earlier part remains in this session's transcript.",
    );
    if listed.is_empty() {
        return text;
    }
    text.push_str("\n\nThe user's requests in the earlier part, oldest first");
    let omitted = requests.len() - listed.len();
    if omitted > 0 {
        text.push_str(&format!(" ({omitted} earlier requests are not listed)"));
    }
    text.push_str(":\n\n");
    text.push_str(&listed.join("\n\n---\n\n"));
    text
}

/// `text` cut to `REQUEST` bytes at a character boundary, with an ellipsis when cut.
fn shorten(text: &str) -> String {
    if text.len() <= REQUEST {
        return text.to_owned();
    }
    let mut end = REQUEST;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Bytes of the conversation records after the last compaction boundary of native Claude rows.
pub(super) fn after_boundary(rows: &[Value]) -> usize {
    let start = rows.iter().rposition(|r| r["subtype"] == "compact_boundary").map_or(0, |i| i + 1);
    records(&rows[start..])
}

/// Bytes of the conversation records among native Claude rows.
pub(super) fn records(rows: &[Value]) -> usize {
    rows.iter()
        .filter(|r| matches!(r["type"].as_str(), Some("user" | "assistant")) && r["isSidechain"] != true)
        .map(|r| match &r["message"]["content"] {
            Value::String(text) => text.len(),
            other => other.to_string().len(),
        })
        .sum()
}
