//! The one place this view knows which module owns a call and how that module
//! shapes its answers: module names, query payloads, reply fields, and the
//! live stream whose movement means the roster may have changed. Everything
//! outside this file speaks in [`Seat`]s and [`MembersPage`]s.
//!
//! TODO(call-module): when ducktape-modules publishes the call module the
//! roster query and the live subscription move chat→call, the channel record
//! stays in chat, and join/leave go to the call module (the view emits no
//! join/leave today — the hub join is the host's). Only this file and its
//! pins change; the shapes are whatever the module publishes, not guessed here.
use serde_json::{Value, json};

/// One committed seat in the call's roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    /// The identity of who sits there, in the module's own spelling.
    pub party: String,
    /// The node the seat's media is fanned out to.
    pub node: String,
}

/// One page of the channel's membership, in the module's own spelling of
/// identities; `next` is the cursor to ask for the following page, or `None`
/// at the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembersPage {
    pub parties: Vec<String>,
    pub next: Option<String>,
}

/// The `rpc.live` target whose movement means the roster may have changed.
/// Will move to the call module.
pub fn roster_live() -> &'static [u8] {
    b"chat"
}

/// The channel record: its name today, and the roster with it.
/// The record stays in chat; the roster will move to the call module.
pub async fn room(channel: &str) -> Result<(String, Vec<Seat>), String> {
    let reply = request("rpc.view", channel_query(channel)).await?;
    Ok((read_name(&reply)?, read_roster(&reply)?))
}

/// The committed roster alone — the session's fan-out set.
/// Will move to the call module.
pub async fn roster(channel: &str) -> Result<Vec<Seat>, String> {
    read_roster(&request("rpc.view", channel_query(channel)).await?)
}

/// One page of the channel's members, from the cursor `after`.
/// Stays in chat.
pub async fn members(channel: &str, after: Option<&str>) -> Result<MembersPage, String> {
    read_members(&request("rpc.view", members_query(channel, after)).await?)
}

pub(crate) async fn request(kind: &str, payload: Value) -> Result<Value, String> {
    let bytes =
        ducktape_view_guest::host::request(kind, &serde_json::to_vec(&payload).expect("query"))
            .await
            .map_err(ducktape_view_guest::host::said)?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn channel_query(channel: &str) -> Value {
    json!({"target": "chat", "query": {"channel": {"channel_id": channel}}})
}

fn members_query(channel: &str, after: Option<&str>) -> Value {
    json!({"target": "chat", "query": {"members": {"channel_id": channel, "after": after, "limit": 128}}})
}

fn read_name(reply: &Value) -> Result<String, String> {
    reply["channel"]["name"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "missing call room".into())
}

fn read_roster(reply: &Value) -> Result<Vec<Seat>, String> {
    reply["channel"]["huddle"]
        .as_array()
        .ok_or("missing call roster")?
        .iter()
        .map(|seat| {
            Ok(Seat {
                party: seat["party"]
                    .as_str()
                    .ok_or("missing call identity")?
                    .to_owned(),
                node: seat["node"].as_str().ok_or("missing call node")?.to_owned(),
            })
        })
        .collect()
}

fn read_members(reply: &Value) -> Result<MembersPage, String> {
    let page = &reply["members"];
    let parties = page["members"]
        .as_array()
        .ok_or("missing members page")?
        .iter()
        .map(|row| {
            row["party"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "missing member identity".into())
        })
        .collect::<Result<_, String>>()?;
    let next = match page["has_more"].as_bool() {
        Some(true) => Some(
            page["next_after"]
                .as_str()
                .ok_or("missing members cursor")?
                .to_owned(),
        ),
        _ => None,
    };
    Ok(MembersPage { parties, next })
}

/// What a test host answers with, in the module's shape, so a shape change
/// moves the fixtures with the readers.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    /// Whether `query` is this view's roster query for `channel`.
    pub fn is_roster_query(query: &Value, channel: &str) -> bool {
        *query == channel_query(channel)
    }

    /// The channel record answer: named `name`, every node seated as `party`.
    pub fn room_reply(name: &str, party: &str, nodes: &[String]) -> Value {
        let seats: Vec<Value> = nodes
            .iter()
            .map(|node| json!({"party": party, "node": node}))
            .collect();
        json!({"channel": {"name": name, "huddle": seats}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_moves_on_the_chat_live_stream() {
        assert_eq!(roster_live(), b"chat");
    }

    #[test]
    fn the_channel_query_asks_chat_for_the_channel_record() {
        assert_eq!(
            channel_query("room"),
            json!({"target": "chat", "query": {"channel": {"channel_id": "room"}}})
        );
    }

    #[test]
    fn the_members_query_asks_chat_for_a_page_after_a_cursor() {
        assert_eq!(
            members_query("room", None),
            json!({"target": "chat", "query": {"members": {"channel_id": "room", "after": null, "limit": 128}}})
        );
        assert_eq!(
            members_query("room", Some("c1"))["query"]["members"]["after"],
            "c1"
        );
    }

    #[test]
    fn the_name_is_read_from_the_channel_record() {
        assert_eq!(
            read_name(&json!({"channel": {"name": "room"}})).unwrap(),
            "room"
        );
        assert_eq!(read_name(&json!({})).unwrap_err(), "missing call room");
    }

    #[test]
    fn the_roster_is_read_from_the_channel_records_huddle() {
        let reply = fixture::room_reply("room", "acct:1", &["node-a".to_owned()]);
        assert_eq!(
            read_roster(&reply).unwrap(),
            [Seat {
                party: "acct:1".into(),
                node: "node-a".into()
            }]
        );
        assert_eq!(
            read_roster(&json!({"channel": {"name": "room"}})).unwrap_err(),
            "missing call roster"
        );
        assert_eq!(
            read_roster(&json!({"channel": {"huddle": [{"party": "acct:1"}]}})).unwrap_err(),
            "missing call node"
        );
    }

    #[test]
    fn a_members_page_is_read_with_its_cursor() {
        let more = json!({"members": {"members": [{"party": "acct:1"}, {"party": "acct:2"}], "has_more": true, "next_after": "c2"}});
        assert_eq!(
            read_members(&more).unwrap(),
            MembersPage {
                parties: vec!["acct:1".into(), "acct:2".into()],
                next: Some("c2".into())
            }
        );
        let last = json!({"members": {"members": [], "has_more": false}});
        assert_eq!(read_members(&last).unwrap().next, None);
        assert_eq!(
            read_members(&json!({"members": {"members": [], "has_more": true}})).unwrap_err(),
            "missing members cursor"
        );
        assert_eq!(
            read_members(&json!({})).unwrap_err(),
            "missing members page"
        );
    }

    #[test]
    fn the_fixture_recognises_the_roster_query() {
        assert!(fixture::is_roster_query(&channel_query("room"), "room"));
        assert!(!fixture::is_roster_query(&channel_query("other"), "room"));
    }
}
