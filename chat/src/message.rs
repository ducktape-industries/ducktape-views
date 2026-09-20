//! Chat's small reader for the committed message format.
//!
//! Composer owns Markdown parsing for outgoing messages. Chat only decodes the
//! block, mark, and party fields it renders or uses for notification policy.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Party {
    Account(u64),
    Key(Vec<u8>),
    Module(String),
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mark {
    Bold,
    Italic,
    Link(String),
    Mention(Party),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Span {
    pub(crate) text: String,
    pub(crate) marks: Vec<Mark>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Block {
    Paragraph(Vec<Span>),
    Quote(Vec<Span>),
    Code(serde_json::Value),
    Divider,
}

/// Resolve the key parties assigned by the committed write. This is the
/// notification reader's identity fold, not the composer's outgoing parser.
pub(crate) fn resolve_assigned_mentions(
    mut blocks: Vec<Block>,
    key_mentions: &[u64],
) -> Result<Vec<Block>, String> {
    let mut accounts = key_mentions.iter();
    let mut resolved = BTreeMap::new();
    for block in &mut blocks {
        let spans = match block {
            Block::Paragraph(spans) | Block::Quote(spans) => spans,
            Block::Code(_) | Block::Divider => continue,
        };
        for span in spans {
            for mark in &mut span.marks {
                let Mark::Mention(Party::Key(key)) = mark else {
                    continue;
                };
                let account = match resolved.get(key) {
                    Some(account) => *account,
                    None => {
                        let account = *accounts.next().ok_or("missing assigned mention account")?;
                        if account == 0 {
                            return Err("assigned mention account is zero".into());
                        }
                        resolved.insert(key.clone(), account);
                        account
                    }
                };
                *mark = Mark::Mention(Party::Account(account));
            }
        }
    }
    if accounts.next().is_some() {
        return Err("unused assigned mention accounts".into());
    }
    Ok(blocks)
}

/// Replace canonical mention tokens in search text while leaving fenced code
/// literal. Chat needs this display helper, not the composer's Markdown parser.
pub(crate) fn draft_mentions(text: &str, label: impl Fn(&Party) -> String) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut display = String::new();
    let mut index = 0;
    while index < chars.len() {
        if let Some(consumed) = code_fence_len(&chars, index) {
            display.extend(&chars[index..index + consumed]);
            index += consumed;
            continue;
        }
        if let Some((party, consumed)) = mention_at(&chars, index) {
            display.push_str(&label(&party));
            index += consumed;
        } else {
            display.push(chars[index]);
            index += 1;
        }
    }
    display
}

fn mention_at(chars: &[char], at: usize) -> Option<(Party, usize)> {
    if chars.get(at) != Some(&'<') || chars.get(at + 1) != Some(&'@') {
        return None;
    }
    let end = chars[at + 2..].iter().position(|c| *c == '>')? + at + 2;
    let id: String = chars[at + 2..end].iter().collect();
    let party = match id.strip_prefix("key:") {
        Some(key) => Party::Key(hex_bytes(key)?),
        None => {
            let decimal = !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit());
            if !decimal {
                return None;
            }
            Party::Account(id.parse().ok()?)
        }
    };
    Some((party, end + 1 - at))
}

fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
        .collect()
}

fn code_fence_len(chars: &[char], at: usize) -> Option<usize> {
    let line_start = at == 0 || chars[at - 1] == '\n';
    if !line_start {
        return None;
    }
    let mut lines = chars[at..].split_inclusive(|ch| *ch == '\n');
    let opener = lines.next()?;
    let opener_text: String = opener.iter().collect();
    if !opener_text.trim().starts_with("```") {
        return None;
    }
    let mut consumed = opener.len();
    for line in lines {
        consumed += line.len();
        let line_text: String = line.iter().collect();
        if line_text.trim() == "```" {
            break;
        }
    }
    Some(consumed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_decoder_reads_the_committed_producer_fixture() {
        let blocks: Vec<Block> = serde_json::from_str(include_str!(
            "../../support/composer/tests/fixtures/message-rich.json"
        ))
        .expect("SDK chat-message fixture decodes locally");
        let Block::Paragraph(spans) = &blocks[0] else {
            panic!("first fixture block is not a paragraph");
        };
        assert_eq!(spans[0].text, "Hello ");
        assert!(
            spans
                .iter()
                .any(|span| { span.marks.iter().any(|mark| matches!(mark, Mark::Bold)) })
        );
        assert!(spans.iter().any(|span| {
            span.marks
                .iter()
                .any(|mark| matches!(mark, Mark::Mention(Party::Account(7))))
        }));
    }
}
