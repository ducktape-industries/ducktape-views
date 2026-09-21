//! Naming and folding: the network's name directory, the reader's own keys,
//! index rows folded into the rows the frame draws, mention candidates and
//! the derived DM channel id. Everything is display logic over `wire`.
use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::chat::{Block, MsgRow, Party, hex, party_handle, unhex};

/// The account bound to a user key: its number (the identity) and its name.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundAccount {
    pub number: u64,
    pub name: String,
}

/// The network's name directory: the account behind each user key (by key
/// hex), every account's name, and which accounts are software. Names are
/// display text, not identity: "the same person" is the account number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameDirectory {
    accounts: BTreeMap<String, BoundAccount>,
    by_account: BTreeMap<u64, String>,
    programs: BTreeSet<u64>,
}

impl NameDirectory {
    pub const fn empty() -> Self {
        Self {
            accounts: BTreeMap::new(),
            by_account: BTreeMap::new(),
            programs: BTreeSet::new(),
        }
    }

    /// From a roster read off the wire: `(number, name, program-controlled,
    /// key bytes)` per account. Keyless programs are named too.
    pub fn from_roster(
        accounts: impl IntoIterator<Item = (u64, String, bool, Vec<Vec<u8>>)>,
    ) -> Self {
        let mut names = Self::empty();
        for (number, name, program, keys) in accounts {
            names.by_account.insert(number, name.clone());
            if program {
                names.programs.insert(number);
            }
            for key in keys {
                let bound = BoundAccount {
                    number,
                    name: name.clone(),
                };
                names.accounts.insert(hex(&key), bound);
            }
        }
        names
    }

    pub fn account_of(&self, key_hex: &str) -> Option<u64> {
        self.accounts.get(key_hex).map(|account| account.number)
    }

    /// A member's label: the bound name, else the shortened key. Takes a
    /// handle (`acct:n`, `user:hex`) or a bare key hex.
    pub fn member_label(&self, key_hex: &str) -> String {
        let handle = if key_hex.contains(':') {
            key_hex.to_string()
        } else {
            format!("user:{key_hex}")
        };
        self.of_handle(&handle)
            .map_or_else(|| short_label(key_hex), str::to_string)
    }

    pub fn party_of(&self, key: &[u8]) -> Party {
        match self.account_of(&hex(key)) {
            Some(account) => Party::Account(account),
            None => Party::Key(key.to_vec()),
        }
    }

    pub fn handle_of(&self, key: &[u8]) -> String {
        party_handle(&self.party_of(key))
    }

    /// An account row belongs to the account's current keys; a key row only
    /// to that exact key.
    pub fn owns_handle(&self, handle: &str, key: &[u8]) -> bool {
        handle == self.handle_of(key) || handle == party_handle(&Party::Key(key.to_vec()))
    }

    fn of_handle(&self, handle: &str) -> Option<&str> {
        match handle.split_once(':') {
            Some(("acct", number)) => self
                .by_account
                .get(&number.parse::<u64>().ok()?)
                .map(String::as_str),
            Some(("user", key)) => self.accounts.get(key).map(|account| account.name.as_str()),
            _ => None,
        }
    }
}

/// The reader: the key they sign with (what `by me` hangs on) and the
/// directory every author is named through.
#[derive(Clone, Copy, Debug)]
pub struct ChatReader<'a> {
    pub key: Option<&'a [u8]>,
    pub names: &'a NameDirectory,
}

impl<'a> ChatReader<'a> {
    pub fn new(key: Option<&'a [u8]>, names: &'a NameDirectory) -> Self {
        Self { key, names }
    }

    pub fn is_me(&self, handle: &str) -> bool {
        self.key
            .is_some_and(|key| self.names.owns_handle(handle, key))
    }
}

/// One message as the frame draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub seq: u64,
    pub author: String,
    pub meta: String,
    pub blocks: Vec<ChatBlock>,
    pub deleted: bool,
    pub reply_count: u64,
    /// Opens a run: the first message, or one whose author differs from the
    /// one above (see [`mark_message_groups`]).
    pub show_author: bool,
    pub reactions: Vec<ChatReaction>,
}

/// `kind` is `paragraph` | `code` | `quote` | `divider`; `text` its flat text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatBlock {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatReaction {
    pub emoji: String,
    pub count: u64,
    pub reacted_by_me: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMember {
    pub key: String,
    pub label: String,
}

pub fn chat_message(row: MsgRow, names: &NameDirectory) -> ChatMessage {
    let meta = if row.rev > 0 {
        format!("#{} · edited", row.seq)
    } else {
        format!("#{}", row.seq)
    };
    let blocks = if row.deleted {
        vec![ChatBlock {
            kind: "paragraph".into(),
            text: "Message deleted".into(),
        }]
    } else {
        row.blocks
            .iter()
            .map(|block| block_view(block, names))
            .collect()
    };
    ChatMessage {
        seq: row.seq,
        author: author_display(&row.author, names),
        meta,
        blocks,
        deleted: row.deleted,
        reply_count: row.reply_count,
        show_author: true,
        reactions: row
            .reactions
            .into_iter()
            .map(|reaction| ChatReaction {
                emoji: reaction.emoji,
                count: reaction.count,
                reacted_by_me: reaction.reacted_by_me,
            })
            .collect(),
    }
}

/// Slack-style grouping: a message shows its author header only when it
/// opens a run. Deleted messages always break a run.
pub fn mark_message_groups(messages: &mut [ChatMessage]) {
    for index in 0..messages.len() {
        messages[index].show_author = index == 0
            || messages[index].deleted
            || messages[index - 1].deleted
            || messages[index - 1].author != messages[index].author;
    }
}

fn block_view(block: &Block, names: &NameDirectory) -> ChatBlock {
    let (kind, text) = match block {
        Block::Paragraph(spans) => ("paragraph", span_text(spans, names)),
        Block::Quote(spans) => ("quote", span_text(spans, names)),
        Block::Code { lang, text } => (
            "code",
            match lang {
                Some(lang) => format!("{lang}\n{text}"),
                None => text.clone(),
            },
        ),
        Block::Divider => ("divider", String::new()),
    };
    ChatBlock {
        kind: kind.into(),
        text,
    }
}

/// Spans to text; a mention plate shows the account's current name.
fn span_text(spans: &[crate::chat::Span], names: &NameDirectory) -> String {
    use crate::chat::Mark;
    spans
        .iter()
        .map(|span| {
            let mention = span.marks.iter().find_map(|mark| match mark {
                Mark::Mention(party) => Some(party),
                _ => None,
            });
            match mention {
                Some(party) => mention_label(party, names),
                None => span.text.clone(),
            }
        })
        .collect()
}

/// An author handle (`user:{hex}`, `acct:{n}`, `module:{id}`, `system`) as
/// its label: the bound name when the directory knows it, else a plain
/// rendering of the handle.
pub fn author_display(author: &str, names: &NameDirectory) -> String {
    names.of_handle(author).map_or_else(
        || match author.split_once(':') {
            Some(("user", id)) => format!("user {}", short_label(id)),
            Some(("acct", account)) => format!("account {account}"),
            Some(("module", id)) => id.to_string(),
            _ => "system".into(),
        },
        str::to_string,
    )
}

pub fn short_label(id: &str) -> String {
    let mut label: String = id.chars().take(8).collect();
    if id.chars().count() > 8 {
        label.push('…');
    }
    label
}

/// Autocomplete candidates: every named account plus the room's unregistered
/// key members, labelled without the `@`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionChoice {
    pub label: String,
    pub party: Party,
}

pub fn mention_choices(names: &NameDirectory, members: &[ChatMember]) -> Vec<MentionChoice> {
    let mut targets: Vec<MentionChoice> = names
        .by_account
        .keys()
        .map(|account| choice(Party::Account(*account), names))
        .collect();
    for member in members {
        let party = match member.key.strip_prefix("acct:") {
            Some(number) => match number.parse::<u64>() {
                Ok(number) => Party::Account(number),
                Err(_) => continue,
            },
            None => {
                let key = member.key.strip_prefix("user:").unwrap_or(&member.key);
                let key = unhex(key).unwrap_or_else(|| key.as_bytes().to_vec());
                if names.account_of(&hex(&key)).is_some() {
                    continue;
                }
                Party::Key(key)
            }
        };
        if !targets.iter().any(|target| target.party == party) {
            targets.push(choice(party, names));
        }
    }
    targets.sort_by_key(|choice| choice.label.to_lowercase());
    targets
}

fn choice(party: Party, names: &NameDirectory) -> MentionChoice {
    MentionChoice {
        label: mention_label(&party, names)[1..].to_string(),
        party,
    }
}

/// The canonical token the composer inserts: `<@account>` or `<@key:hex>`.
pub fn mention_token(party: &Party) -> String {
    match party {
        Party::Account(account) => format!("<@{account}>"),
        Party::Key(key) => format!("<@key:{}>", hex(key)),
        Party::Module(_) | Party::System => String::new(),
    }
}

pub fn mention_label(party: &Party, names: &NameDirectory) -> String {
    match party {
        Party::Account(account) => names
            .by_account
            .get(account)
            .filter(|name| !name.is_empty())
            .map_or_else(|| format!("@account-{account}"), |name| format!("@{name}")),
        Party::Key(key) => format!("@{}", names.member_label(&hex(key))),
        Party::Module(module) => format!("@{module}"),
        Party::System => "@system".into(),
    }
}

/// The two-party channel id for a pair of accounts, sorted so both ends
/// derive the same id: `dm-` + sha256(low, 0x1f, high) as hex. Mirrors the
/// module's derivation (`dm_channel_id` in ducktape-modules).
pub fn dm_channel_id(a: &str, b: &str) -> String {
    let (low, high) = if a <= b { (a, b) } else { (b, a) };
    let mut digest = Sha256::new();
    digest.update(low.as_bytes());
    digest.update([0x1f]);
    digest.update(high.as_bytes());
    format!("dm-{}", hex(&digest.finalize()))
}
