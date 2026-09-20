//! Call invitation submission through the generic host contract.
use super::Send;
pub use super::Target;
use ducktape_view_guest::host;

pub use ducktape_view_guest::host::said;

pub async fn id() -> Result<String, host::Refusal> {
    let bytes = host::request("host.id", b"message").await?;
    String::from_utf8(bytes).map_err(|error| host::malformed(error.to_string()))
}

pub async fn submit(id: String, send: &Send, target: &Target) -> Result<(), host::Refusal> {
    let body = send.body.clone();
    if body.is_empty() || body.len() > 16 * 1024 {
        return Err(host::Refusal::new(
            "invalid_body",
            "Message must contain between 1 byte and 16 KiB",
        ));
    }
    let blocks = super::message::parse_message(&body);
    let payload = match target {
        Target::Post { channel, thread } => {
            serde_json::json!({"post_message": {"channel_id":channel, "message_id":id, "blocks":blocks, "thread":thread}})
        }
        Target::Edit {
            channel,
            seq,
            base_rev,
        } => {
            serde_json::json!({"edit_message": {"channel_id":channel,"seq":seq,"blocks":blocks,"base_rev":base_rev}})
        }
    };
    let ask = serde_json::json!({"target":"chat", "payload":payload});
    host::request(
        "op.submit",
        &serde_json::to_vec(&ask).expect("message envelope"),
    )
    .await
    .map(|_| ())
}
