use super::*;
use ducktape_view_guest::testing::{answer, has_text, item, press, refuse, type_into};
use ducktape_view_guest::view::Shell;
use ducktape_view_guest::{Driver, wire};

fn request<'a>(frame: &'a wire::Frame, kind: &str) -> impl Iterator<Item = &'a wire::Request> {
    frame
        .requests
        .iter()
        .filter(move |request| request.kind == kind)
}

fn roster_page() -> Vec<u8> {
    json!({"accounts": [{"number": 7, "name": "eddy", "control": "keys", "keys": [{"pubkey": [1, 2]}]}]})
        .to_string()
        .into_bytes()
}

#[test]
fn boot_reads_the_roster_and_the_rooms_and_a_room_opens_on_press() {
    let mut driver = Driver::<Shell<Chat>>::new();
    let frame = driver.tick(vec![]);
    let kinds: Vec<_> = frame.requests.iter().map(|r| r.kind.as_str()).collect();
    assert!(kinds.contains(&"chat.props") && kinds.contains(&"rpc.live"));
    let props = request(&frame, "chat.props").next().unwrap().id;
    let rooms = request(&frame, "rpc.view").next().unwrap().id;
    assert!(has_text(&frame, "Loading rooms…"));

    // the session names the reader, so the roster is read again for them
    let session =
        json!({"me": "acct:7", "me_key": "0102", "connected": true, "network_name": "duck"});
    let frame = driver.tick(vec![item(props, session.to_string().as_bytes())]);
    assert_eq!(frame.cancels.len(), 1);
    let names = request(&frame, "rpc.query").next().unwrap().id;

    let channels = json!({"channels": {"channels": [{"id": "general", "name": "General", "created_at": 0, "post_policy": "open", "owner": "acct:7", "archived": false, "hooks": [], "huddle": [], "voice": false, "head_seq": 3}], "has_more": false, "next_after": null}});
    let frame = driver.tick(vec![
        answer(names, &roster_page()),
        answer(rooms, channels.to_string().as_bytes()),
    ]);
    assert!(has_text(&frame, "General"));
    assert!(has_text(&frame, "Pick a room"));

    let frame = driver.tick(press(&frame, "room/general"));
    assert_eq!(driver.app.state().room.as_ref().unwrap().id, "general");
    let find = |frame: &wire::Frame, word: &str| {
        request(frame, "rpc.view")
            .find(|r| std::str::from_utf8(&r.payload).unwrap().contains(word))
            .unwrap()
            .id
    };
    let (roots, seats) = (find(&frame, "\"roots\""), find(&frame, "\"members\""));
    let page = json!({"roots": {"roots": [{"channel_id": "general", "seq": 1, "message_id": "m1", "author": "acct:7", "height": 1, "time": 0, "blocks": [{"paragraph": [{"text": "hello", "marks": []}]}], "text": "hello", "deleted": false, "edited": false, "rev": 0, "edited_at": null, "base_rev": null, "thread": null, "reply_count": 0, "last_reply_seq": null, "reactions": [], "tags": []}], "has_more": false, "next_before_seq": null}});
    let frame = driver.tick(vec![
        answer(roots, page.to_string().as_bytes()),
        answer(
            seats,
            br#"{"members":{"members":[],"has_more":false,"next_after":null}}"#,
        ),
    ]);
    assert!(
        has_text(&frame, "hello"),
        "{:?}",
        ducktape_view_guest::testing::texts(&frame)
    );
    assert!(has_text(&frame, "eddy"));

    // creating a channel asks for an id, then submits the op
    let frame = driver.tick(type_into(&frame, "channel-name", "random"));
    let frame = driver.tick(press(&frame, "create-channel"));
    let id = request(&frame, "host.id").next().unwrap().id;
    let frame = driver.tick(vec![answer(id, b"chan-1")]);
    let submit = request(&frame, "op.submit").next().unwrap();
    assert!(
        std::str::from_utf8(&submit.payload)
            .unwrap()
            .contains("create_channel")
    );
    let frame = driver.tick(vec![refuse(submit.id, "no")]);
    assert!(has_text(&frame, "no"));

    // a snapshot restores with the room and reopens it
    let bytes = driver.snapshot().unwrap();
    let mut restored = Driver::<Shell<Chat>>::from_snapshot(&bytes, false).unwrap();
    let frame = restored.tick(vec![]);
    assert_eq!(restored.app.state().room.as_ref().unwrap().id, "general");
    assert!(request(&frame, "rpc.view").count() >= 2);
}
