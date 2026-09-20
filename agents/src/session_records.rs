//! View-local adapter for Core's candidate machine-session records query.
//!
//! This module deliberately stops at decoding and bounded folding. It does
//! not issue a request, persist records, or turn historical events into
//! control authority. The wire shapes mirror Core commit
//! `6e8a9673e9ead64139ff92fffca130fa3069037d` locally so the view does not
//! depend on a domain or wire crate.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const QUERY_ROUTE: &str = "/v1/run-records/query";
pub const DEFAULT_SESSION_PAGE: usize = 50;
pub const MAX_SESSION_PAGE: usize = 100;
pub const DEFAULT_EVENT_PAGE: usize = 100;
pub const MAX_EVENT_PAGE: usize = 500;

const MAX_SESSION_PAGES: usize = 32;
const MAX_FOLDED_SESSIONS: usize = MAX_SESSION_PAGES * MAX_SESSION_PAGE;
const MAX_HISTORY_PAGES: usize = 32;
const MAX_HISTORY_EVENTS: usize = MAX_HISTORY_PAGES * MAX_EVENT_PAGE;
const MAX_RECENT_EVENTS: usize = 128;

/// Core's exact status vocabulary. Unknown statuses are not silently mapped.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionStatus {
    Active,
    Completed,
    Failed,
    Interrupted,
    Crashed,
}

/// The request origin is separate from requester identity.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionOrigin {
    User,
    Mention,
    Dm,
    Delegation,
}

/// How the provider session was invoked, independent of its origin.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationKind {
    Pty,
    Sched,
    Runs,
}

/// A non-secret requester identity from Core.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequesterPrincipal {
    External { principal_id: String },
    Program { account_id: u64 },
    Module { module_id: String },
    System,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoreIdentity {
    pub machine_id: String,
    pub network_id: String,
}

/// A provider session. `session_id` is the provider's actual session ID;
/// `run_id` is an explicit optional mapping and is never inferred from a
/// parent session.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub run_id: Option<String>,
    pub agent_id: Option<String>,
    pub origin: Option<SessionOrigin>,
    pub requester: Option<RequesterPrincipal>,
    pub model: Option<String>,
    pub executor: Option<String>,
    pub invocation_kind: Option<InvocationKind>,
    pub status: SessionStatus,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub owner_account_id: Option<u64>,
    pub machine_id: String,
    pub network_id: String,
}

/// An event remains generic JSON so unknown provider payloads stay readable
/// historical data without being interpreted or stored as controls.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SessionEvent {
    #[serde(default)]
    pub seq: u64,
    pub event_id: String,
    pub run_id: Option<String>,
    pub at: Option<String>,
    pub kind: String,
    pub stream: Option<String>,
    pub payload: Value,
}

/// Core's candidate request. The tag and variant names intentionally remain
/// exact; consumers must not invent a second query vocabulary.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunRecordsQuery {
    Sessions {
        #[serde(default)]
        run_id: Option<String>,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    SessionForRun {
        run_id: String,
    },
    Events {
        session_id: String,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        tail: bool,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionsReply {
    pub kind: String,
    pub identity: StoreIdentity,
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventsReply {
    pub kind: String,
    pub identity: StoreIdentity,
    pub session_id: String,
    pub events: Vec<SessionEvent>,
    pub end_cursor: String,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    Json(String),
    InvalidQuery(&'static str),
    MalformedReply(&'static str),
    IdentityMismatch,
    SessionMismatch,
    RunMismatch,
    DuplicateSession(String),
    DuplicateEvent(String),
    PageAfterComplete,
    PageLimit,
    ItemLimit,
    CursorDidNotAdvance,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "machine-session JSON: {error}"),
            Self::InvalidQuery(reason) => write!(f, "invalid machine-session query: {reason}"),
            Self::MalformedReply(reason) => {
                write!(f, "malformed machine-session reply: {reason}")
            }
            Self::IdentityMismatch => f.write_str("machine-session identity mismatch"),
            Self::SessionMismatch => f.write_str("machine-session ID mismatch"),
            Self::RunMismatch => f.write_str("machine-session run mapping mismatch"),
            Self::DuplicateSession(id) => write!(f, "duplicate machine session: {id}"),
            Self::DuplicateEvent(id) => write!(f, "duplicate machine-session event: {id}"),
            Self::PageAfterComplete => f.write_str("page received after pagination completed"),
            Self::PageLimit => f.write_str("machine-session page accumulation limit exceeded"),
            Self::ItemLimit => f.write_str("machine-session item accumulation limit exceeded"),
            Self::CursorDidNotAdvance => f.write_str("machine-session cursor did not advance"),
        }
    }
}

impl std::error::Error for AdapterError {}

/// Validate and encode one Core candidate request body. Cursor strings are
/// carried opaquely and are never parsed by the view.
pub fn encode_query(query: &RunRecordsQuery) -> Result<Value, AdapterError> {
    validate_query(query)?;
    serde_json::to_value(query).map_err(|error| AdapterError::Json(error.to_string()))
}

pub fn encode_query_bytes(query: &RunRecordsQuery) -> Result<Vec<u8>, AdapterError> {
    let body = encode_query(query)?;
    serde_json::to_vec(&body).map_err(|error| AdapterError::Json(error.to_string()))
}

pub fn decode_sessions_reply(bytes: &[u8]) -> Result<SessionsReply, AdapterError> {
    let reply: SessionsReply =
        serde_json::from_slice(bytes).map_err(|error| AdapterError::Json(error.to_string()))?;
    reply.validate()?;
    Ok(reply)
}

pub fn decode_events_reply(bytes: &[u8]) -> Result<EventsReply, AdapterError> {
    let reply: EventsReply =
        serde_json::from_slice(bytes).map_err(|error| AdapterError::Json(error.to_string()))?;
    reply.validate()?;
    Ok(reply)
}

impl SessionsReply {
    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.kind != "sessions" {
            return Err(AdapterError::MalformedReply("wrong reply kind"));
        }
        validate_identity(&self.identity)?;
        if self.sessions.len() > MAX_SESSION_PAGE {
            return Err(AdapterError::MalformedReply(
                "too many sessions in one page",
            ));
        }
        if self.has_more != self.next_cursor.is_some() {
            return Err(AdapterError::MalformedReply("inconsistent session cursor"));
        }
        for session in &self.sessions {
            validate_summary(session)?;
            if session.machine_id != self.identity.machine_id
                || session.network_id != self.identity.network_id
            {
                return Err(AdapterError::IdentityMismatch);
            }
        }
        Ok(())
    }
}

impl EventsReply {
    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.kind != "events" {
            return Err(AdapterError::MalformedReply("wrong reply kind"));
        }
        validate_identity(&self.identity)?;
        validate_session_id(&self.session_id)
            .map_err(|_| AdapterError::MalformedReply("invalid event session ID"))?;
        if self.events.len() > MAX_EVENT_PAGE {
            return Err(AdapterError::MalformedReply("too many events in one page"));
        }
        if self.end_cursor.is_empty() {
            return Err(AdapterError::MalformedReply("missing event end cursor"));
        }
        if self.has_more && self.events.is_empty() {
            return Err(AdapterError::MalformedReply(
                "empty event page marked has_more",
            ));
        }
        validate_event_page(&self.events)?;
        Ok(())
    }
}

/// Bounded state for the sessions list lane. A reset is required before
/// changing the run filter, preventing pages from different queries mixing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionListState {
    identity: StoreIdentity,
    run_id: Option<String>,
    sessions: Vec<SessionSummary>,
    next_cursor: Option<String>,
    has_more: bool,
    pages: usize,
}

impl SessionListState {
    pub fn new(identity: StoreIdentity, run_id: Option<String>) -> Result<Self, AdapterError> {
        validate_identity(&identity)?;
        if let Some(run_id) = run_id.as_deref() {
            validate_run_id(run_id).map_err(|_| AdapterError::InvalidQuery("invalid run_id"))?;
        }
        Ok(Self {
            identity,
            run_id,
            sessions: Vec::new(),
            next_cursor: None,
            has_more: false,
            pages: 0,
        })
    }

    pub fn next_query(&self) -> Result<Option<RunRecordsQuery>, AdapterError> {
        if self.pages != 0 && !self.has_more {
            return Ok(None);
        }
        let query = RunRecordsQuery::Sessions {
            run_id: self.run_id.clone(),
            after: self.next_cursor.clone(),
            limit: Some(DEFAULT_SESSION_PAGE),
        };
        encode_query(&query)?;
        Ok(Some(query))
    }

    pub fn fold(&mut self, page: SessionsReply) -> Result<(), AdapterError> {
        page.validate()?;
        if page.identity != self.identity {
            return Err(AdapterError::IdentityMismatch);
        }
        if self.pages != 0 && !self.has_more {
            return Err(AdapterError::PageAfterComplete);
        }
        if self.pages >= MAX_SESSION_PAGES {
            return Err(AdapterError::PageLimit);
        }
        if self.has_more && self.next_cursor == page.next_cursor {
            return Err(AdapterError::CursorDidNotAdvance);
        }
        if self.sessions.len() + page.sessions.len() > MAX_FOLDED_SESSIONS {
            return Err(AdapterError::ItemLimit);
        }

        let mut known: BTreeSet<&str> = self
            .sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        for session in &page.sessions {
            if self
                .run_id
                .as_deref()
                .is_some_and(|run_id| session.run_id.as_deref() != Some(run_id))
            {
                return Err(AdapterError::RunMismatch);
            }
            if !known.insert(session.session_id.as_str()) {
                return Err(AdapterError::DuplicateSession(session.session_id.clone()));
            }
        }

        self.sessions.extend(page.sessions);
        self.next_cursor = page.next_cursor;
        self.has_more = page.has_more;
        self.pages += 1;
        Ok(())
    }

    pub fn reset(&mut self, run_id: Option<String>) -> Result<(), AdapterError> {
        let replacement = Self::new(self.identity.clone(), run_id)?;
        *self = replacement;
        Ok(())
    }

    pub fn identity(&self) -> &StoreIdentity {
        &self.identity
    }

    pub fn sessions(&self) -> &[SessionSummary] {
        &self.sessions
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryState {
    pub events: Vec<SessionEvent>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pages: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentTailState {
    pub events: Vec<SessionEvent>,
    /// The last durable event cursor, reusable as the next tail `after`.
    pub high_water: Option<String>,
    /// This is display state only. It is never a continuation cursor.
    pub older_omitted: bool,
}

/// Independent history and recent-tail folds for one provider session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionEventsState {
    identity: StoreIdentity,
    session_id: String,
    run_id: Option<String>,
    pub history: HistoryState,
    pub recent_tail: RecentTailState,
}

impl SessionEventsState {
    pub fn new(identity: StoreIdentity, summary: &SessionSummary) -> Result<Self, AdapterError> {
        validate_identity(&identity)?;
        validate_summary(summary)?;
        if summary.machine_id != identity.machine_id || summary.network_id != identity.network_id {
            return Err(AdapterError::IdentityMismatch);
        }
        Ok(Self {
            identity,
            session_id: summary.session_id.clone(),
            run_id: summary.run_id.clone(),
            history: HistoryState::default(),
            recent_tail: RecentTailState::default(),
        })
    }

    pub fn history_query(&self) -> Result<Option<RunRecordsQuery>, AdapterError> {
        if self.history.pages != 0 && !self.history.has_more {
            return Ok(None);
        }
        let query = RunRecordsQuery::Events {
            session_id: self.session_id.clone(),
            after: self.history.next_cursor.clone(),
            limit: Some(DEFAULT_EVENT_PAGE),
            tail: false,
        };
        encode_query(&query)?;
        Ok(Some(query))
    }

    /// Tail polling is independent from historical pagination. `has_more`
    /// from a tail response is represented as `older_omitted`, never as a
    /// forward request condition.
    pub fn recent_tail_query(&self) -> Result<RunRecordsQuery, AdapterError> {
        let query = RunRecordsQuery::Events {
            session_id: self.session_id.clone(),
            after: self.recent_tail.high_water.clone(),
            limit: Some(DEFAULT_EVENT_PAGE),
            tail: true,
        };
        encode_query(&query)?;
        Ok(query)
    }

    pub fn fold_history(&mut self, page: EventsReply) -> Result<(), AdapterError> {
        self.validate_event_reply(&page)?;
        if self.history.pages != 0 && !self.history.has_more {
            return Err(AdapterError::PageAfterComplete);
        }
        if self.history.pages >= MAX_HISTORY_PAGES {
            return Err(AdapterError::PageLimit);
        }
        if self.history.has_more && self.history.next_cursor == Some(page.end_cursor.clone()) {
            return Err(AdapterError::CursorDidNotAdvance);
        }
        if self.history.events.len() + page.events.len() > MAX_HISTORY_EVENTS {
            return Err(AdapterError::ItemLimit);
        }

        self.validate_append(&self.history.events, &page.events)?;
        self.history.events.extend(page.events);
        self.history.next_cursor = page.has_more.then_some(page.end_cursor);
        self.history.has_more = page.has_more;
        self.history.pages += 1;
        Ok(())
    }

    pub fn fold_recent_tail(&mut self, page: EventsReply) -> Result<(), AdapterError> {
        self.validate_event_reply(&page)?;
        self.validate_append(&self.recent_tail.events, &page.events)?;

        let mut events = std::mem::take(&mut self.recent_tail.events);
        events.extend(page.events);
        let dropped = events.len().saturating_sub(MAX_RECENT_EVENTS);
        if dropped != 0 {
            events.drain(..dropped);
        }
        self.recent_tail.events = events;
        self.recent_tail.high_water = Some(page.end_cursor);
        self.recent_tail.older_omitted |= page.has_more || dropped != 0;
        Ok(())
    }

    /// Historical event payloads have no control authority by construction.
    pub const fn controls_enabled(&self) -> bool {
        false
    }

    pub fn reset(&mut self) {
        self.history = HistoryState::default();
        self.recent_tail = RecentTailState::default();
    }

    pub fn identity(&self) -> &StoreIdentity {
        &self.identity
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    fn validate_event_reply(&self, page: &EventsReply) -> Result<(), AdapterError> {
        page.validate()?;
        if page.identity != self.identity {
            return Err(AdapterError::IdentityMismatch);
        }
        if page.session_id != self.session_id {
            return Err(AdapterError::SessionMismatch);
        }
        for event in &page.events {
            if let Some(run_id) = event.run_id.as_deref()
                && self.run_id.as_deref() != Some(run_id)
            {
                return Err(AdapterError::RunMismatch);
            }
        }
        Ok(())
    }

    fn validate_append(
        &self,
        existing: &[SessionEvent],
        incoming: &[SessionEvent],
    ) -> Result<(), AdapterError> {
        let mut ids: BTreeSet<&str> = existing
            .iter()
            .map(|event| event.event_id.as_str())
            .collect();
        let mut seq = existing.last().map(|event| event.seq);
        for event in incoming {
            if !ids.insert(event.event_id.as_str()) {
                return Err(AdapterError::DuplicateEvent(event.event_id.clone()));
            }
            if seq.is_some_and(|previous| event.seq <= previous) {
                return Err(AdapterError::CursorDidNotAdvance);
            }
            seq = Some(event.seq);
        }
        Ok(())
    }
}

fn validate_query(query: &RunRecordsQuery) -> Result<(), AdapterError> {
    match query {
        RunRecordsQuery::Sessions {
            run_id,
            after,
            limit,
        } => {
            if let Some(run_id) = run_id {
                validate_run_id(run_id)
                    .map_err(|_| AdapterError::InvalidQuery("invalid run_id"))?;
            }
            validate_cursor(after.as_deref())?;
            validate_limit(*limit, MAX_SESSION_PAGE)?;
        }
        RunRecordsQuery::SessionForRun { run_id } => {
            validate_run_id(run_id).map_err(|_| AdapterError::InvalidQuery("invalid run_id"))?;
        }
        RunRecordsQuery::Events {
            session_id,
            after,
            limit,
            ..
        } => {
            validate_session_id(session_id)
                .map_err(|_| AdapterError::InvalidQuery("invalid session_id"))?;
            validate_cursor(after.as_deref())?;
            validate_limit(*limit, MAX_EVENT_PAGE)?;
        }
    }
    Ok(())
}

fn validate_limit(limit: Option<usize>, maximum: usize) -> Result<(), AdapterError> {
    if let Some(limit) = limit {
        if limit == 0 {
            return Err(AdapterError::InvalidQuery("zero limit"));
        }
        if limit > maximum {
            return Err(AdapterError::InvalidQuery("limit too large"));
        }
    }
    Ok(())
}

fn validate_cursor(cursor: Option<&str>) -> Result<(), AdapterError> {
    if cursor.is_some_and(str::is_empty) {
        return Err(AdapterError::InvalidQuery("empty cursor"));
    }
    Ok(())
}

fn validate_identity(identity: &StoreIdentity) -> Result<(), AdapterError> {
    if identity.machine_id.is_empty() || identity.network_id.is_empty() {
        return Err(AdapterError::MalformedReply("missing store identity"));
    }
    Ok(())
}

fn validate_summary(summary: &SessionSummary) -> Result<(), AdapterError> {
    validate_session_id(&summary.session_id)
        .map_err(|_| AdapterError::MalformedReply("invalid session ID"))?;
    if let Some(parent) = summary.parent_session_id.as_deref() {
        validate_session_id(parent)
            .map_err(|_| AdapterError::MalformedReply("invalid parent session ID"))?;
    }
    if let Some(run_id) = summary.run_id.as_deref() {
        validate_run_id(run_id).map_err(|_| AdapterError::MalformedReply("invalid run_id"))?;
    }
    if summary.machine_id.is_empty() || summary.network_id.is_empty() {
        return Err(AdapterError::MalformedReply("missing summary identity"));
    }
    Ok(())
}

fn validate_event_page(events: &[SessionEvent]) -> Result<(), AdapterError> {
    let mut ids = BTreeSet::new();
    let mut previous = None;
    for event in events {
        if event.seq == 0 || event.event_id.is_empty() || event.kind.is_empty() {
            return Err(AdapterError::MalformedReply("invalid event metadata"));
        }
        if !ids.insert(event.event_id.as_str()) {
            return Err(AdapterError::DuplicateEvent(event.event_id.clone()));
        }
        if previous.is_some_and(|seq| event.seq <= seq) {
            return Err(AdapterError::MalformedReply("events are not ordered"));
        }
        previous = Some(event.seq);
        if let Some(run_id) = event.run_id.as_deref() {
            validate_run_id(run_id).map_err(|_| AdapterError::MalformedReply("invalid run_id"))?;
        }
    }
    Ok(())
}

fn validate_session_id(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(());
    }
    Ok(())
}

fn validate_run_id(value: &str) -> Result<(), ()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RUN_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn identity(machine_id: &str) -> StoreIdentity {
        StoreIdentity {
            machine_id: machine_id.into(),
            network_id: "network-a".into(),
        }
    }

    fn summary(identity: &StoreIdentity, session_id: &str) -> SessionSummary {
        SessionSummary {
            session_id: session_id.into(),
            parent_session_id: None,
            run_id: Some(RUN_ID.into()),
            agent_id: Some("agent-a".into()),
            origin: Some(SessionOrigin::Delegation),
            requester: Some(RequesterPrincipal::Program { account_id: 7 }),
            model: Some("model-a".into()),
            executor: Some("executor-a".into()),
            invocation_kind: Some(InvocationKind::Pty),
            status: SessionStatus::Active,
            started_at: None,
            last_activity_at: Some("2026-09-20T10:00:00Z".into()),
            owner_account_id: Some(7),
            machine_id: identity.machine_id.clone(),
            network_id: identity.network_id.clone(),
        }
    }

    fn event(seq: u64, event_id: &str, run_id: Option<&str>) -> SessionEvent {
        SessionEvent {
            seq,
            event_id: event_id.into(),
            run_id: run_id.map(str::to_owned),
            at: None,
            kind: "future_provider_event".into(),
            stream: Some("provider".into()),
            payload: json!({"new_provider_field": [1, true, null]}),
        }
    }

    fn sessions_page(
        identity: &StoreIdentity,
        sessions: Vec<SessionSummary>,
        next_cursor: Option<&str>,
        has_more: bool,
    ) -> SessionsReply {
        SessionsReply {
            kind: "sessions".into(),
            identity: identity.clone(),
            sessions,
            next_cursor: next_cursor.map(str::to_owned),
            has_more,
        }
    }

    fn events_page(
        identity: &StoreIdentity,
        session_id: &str,
        events: Vec<SessionEvent>,
        end_cursor: &str,
        has_more: bool,
    ) -> EventsReply {
        EventsReply {
            kind: "events".into(),
            identity: identity.clone(),
            session_id: session_id.into(),
            events,
            end_cursor: end_cursor.into(),
            has_more,
        }
    }

    #[test]
    fn query_encoding_matches_candidate_tags_and_keeps_cursors_opaque() {
        let query = RunRecordsQuery::Events {
            session_id: "provider-session".into(),
            after: Some("q1.signature.opaque".into()),
            limit: Some(12),
            tail: true,
        };
        assert_eq!(
            encode_query(&query).unwrap(),
            json!({
                "kind": "events",
                "session_id": "provider-session",
                "after": "q1.signature.opaque",
                "limit": 12,
                "tail": true,
            })
        );
        assert!(
            serde_json::from_value::<RunRecordsQuery>(json!({
                "kind": "made_up",
            }))
            .is_err()
        );
        assert!(
            encode_query(&RunRecordsQuery::SessionForRun {
                run_id: "not-a-run".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn candidate_source_shape_preserves_nullable_fields_and_unknown_payload() {
        let source_shape = json!({
            "kind": "events",
            "identity": {"machine_id": "machine-a", "network_id": "network-a"},
            "session_id": "provider-session",
            "events": [{
                "seq": 1,
                "event_id": "event-1",
                "run_id": RUN_ID,
                "at": null,
                "kind": "future_provider_event",
                "stream": null,
                "payload": {"provider_specific": {"value": true}}
            }],
            "end_cursor": "q1.signed.cursor",
            "has_more": false,
        });
        let reply: EventsReply = serde_json::from_value(source_shape).unwrap();
        reply.validate().unwrap();
        assert_eq!(reply.events[0].at, None);
        assert_eq!(reply.events[0].payload["provider_specific"]["value"], true);
    }

    #[test]
    fn session_pages_require_identity_and_reset_before_a_new_filter() {
        let id = identity("machine-a");
        let first = summary(&id, "session-a");
        let second = summary(&id, "session-b");
        let mut state = SessionListState::new(id.clone(), None).unwrap();
        assert!(matches!(
            state.next_query().unwrap(),
            Some(RunRecordsQuery::Sessions { after: None, .. })
        ));
        state
            .fold(sessions_page(&id, vec![first], Some("opaque-next"), true))
            .unwrap();
        assert!(matches!(
            state.next_query().unwrap(),
            Some(RunRecordsQuery::Sessions { after: Some(cursor), .. }) if cursor == "opaque-next"
        ));
        state
            .fold(sessions_page(&id, vec![second], None, false))
            .unwrap();
        assert!(state.next_query().unwrap().is_none());

        state.reset(Some(RUN_ID.into())).unwrap();
        assert!(matches!(
            state.next_query().unwrap(),
            Some(RunRecordsQuery::Sessions { run_id: Some(run_id), after: None, .. }) if run_id == RUN_ID
        ));

        let wrong = identity("machine-b");
        assert_eq!(
            state
                .fold(sessions_page(&wrong, vec![], None, false))
                .unwrap_err(),
            AdapterError::IdentityMismatch
        );
    }

    #[test]
    fn history_and_tail_have_separate_cursors_and_never_grant_controls() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();

        state
            .fold_history(events_page(
                &id,
                "provider-session",
                vec![event(1, "event-1", Some(RUN_ID)), event(2, "event-2", None)],
                "history-cursor",
                true,
            ))
            .unwrap();
        assert!(matches!(
            state.history_query().unwrap(),
            Some(RunRecordsQuery::Events { after: Some(cursor), tail: false, .. }) if cursor == "history-cursor"
        ));
        state
            .fold_history(events_page(
                &id,
                "provider-session",
                vec![event(3, "event-3", Some(RUN_ID))],
                "history-done",
                false,
            ))
            .unwrap();
        assert!(state.history_query().unwrap().is_none());

        state
            .fold_recent_tail(events_page(
                &id,
                "provider-session",
                vec![event(4, "event-4", Some(RUN_ID))],
                "tail-high-water",
                true,
            ))
            .unwrap();
        assert!(state.recent_tail.older_omitted);
        assert!(matches!(
            state.recent_tail_query().unwrap(),
            RunRecordsQuery::Events { after: Some(cursor), tail: true, .. } if cursor == "tail-high-water"
        ));
        assert!(!state.controls_enabled());
    }

    #[test]
    fn event_pages_reject_wrong_session_identity_and_explicit_run_mapping() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();
        let wrong_id = events_page(&id, "another-session", vec![], "cursor", false);
        assert_eq!(
            state.fold_history(wrong_id).unwrap_err(),
            AdapterError::SessionMismatch
        );

        let wrong_run = event(1, "event-1", Some(&"f".repeat(64)));
        let page = events_page(&id, "provider-session", vec![wrong_run], "cursor", false);
        assert_eq!(
            state.fold_history(page).unwrap_err(),
            AdapterError::RunMismatch
        );
    }
}
