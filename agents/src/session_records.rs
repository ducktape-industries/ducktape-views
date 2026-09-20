//! View-local adapter for Core's candidate machine-session records query.
//!
//! This module deliberately stops at decoding and bounded folding. It does
//! not issue a request, persist records, or turn historical events into
//! control authority. The wire shapes mirror Core commit
//! `6e8a9673e9ead64139ff92fffca130fa3069037d` locally so the view does not
//! depend on a domain or wire crate.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use ducktape_view_guest::host;
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

/// One reply, checked on `bytes.len()` before any decoding: a full event
/// page of 500 events at ~4 KiB each. Larger is a producer fault, not a page.
pub const MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;
/// Sessions retained per list lane: 3200 summaries at ~1 KiB each.
pub const MAX_SESSION_LIST_BYTES: usize = 4 * 1024 * 1024;
/// History retained per open session: one long log, the largest thing a
/// wasm view keeps resident; beyond this the reader narrows or resets.
pub const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;
/// Recent tail retained per open session: 128 events at ~16 KiB each, the
/// same per-event ceiling `host.rs` clips live trace lines to.
pub const MAX_RECENT_TAIL_BYTES: usize = 2 * 1024 * 1024;

/// Request generations are minted process-wide, so a generation from one
/// state never matches another state's current one. Same idea as
/// `LiveRun::subscription_generation` in `host.rs`: A→B→A leaves an old A
/// reply in flight that identity checks alone cannot tell from the new A.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn mint_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// A query together with the generation that asked it. The reply must be
/// folded with this generation; the state rejects any other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FencedQuery {
    pub query: RunRecordsQuery,
    pub request_generation: u64,
}

/// The host answer for one fenced query, split so a refusal stays typed and
/// never reaches a decoder.
pub fn reply_bytes(answer: host::Answer) -> Result<Vec<u8>, AdapterError> {
    answer.map_err(|refusal| AdapterError::Refused {
        reason: refusal.reason,
        sentence: refusal.sentence,
    })
}

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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionsReply {
    pub kind: String,
    pub identity: StoreIdentity,
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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
    /// The reply answers a generation this state is no longer asking for.
    StaleGeneration {
        current: u64,
        answered: u64,
    },
    /// Refused on `bytes.len()` alone, before any decoding.
    ReplyTooLarge {
        bytes: usize,
        limit: usize,
    },
    /// Keeping this page would exceed the lane's retained-byte budget. The
    /// state is unchanged; the caller resets the lane or narrows the query.
    RetainedBudgetExceeded {
        limit: usize,
    },
    /// The host refused the request; nothing was decoded.
    Refused {
        reason: String,
        sentence: String,
    },
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration { current, answered } => write!(
                f,
                "machine-session reply answers request generation {answered}, current is {current}"
            ),
            Self::ReplyTooLarge { bytes, limit } => write!(
                f,
                "machine-session reply of {bytes} bytes exceeds the {limit}-byte reply limit"
            ),
            Self::RetainedBudgetExceeded { limit } => write!(
                f,
                "keeping this page would exceed the {limit}-byte retained budget; reset the lane or narrow the query"
            ),
            Self::Refused { reason, sentence } => {
                write!(f, "machine-session request refused ({reason}): {sentence}")
            }
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

fn check_reply_size(bytes: &[u8]) -> Result<(), AdapterError> {
    if bytes.len() > MAX_REPLY_BYTES {
        return Err(AdapterError::ReplyTooLarge {
            bytes: bytes.len(),
            limit: MAX_REPLY_BYTES,
        });
    }
    Ok(())
}

fn check_generation(current: u64, answered: u64) -> Result<(), AdapterError> {
    if answered != current {
        return Err(AdapterError::StaleGeneration { current, answered });
    }
    Ok(())
}

fn check_budget(retained: usize, incoming: usize, limit: usize) -> Result<(), AdapterError> {
    if retained + incoming > limit {
        return Err(AdapterError::RetainedBudgetExceeded { limit });
    }
    Ok(())
}

pub fn decode_sessions_reply(bytes: &[u8]) -> Result<SessionsReply, AdapterError> {
    check_reply_size(bytes)?;
    let reply: SessionsReply =
        serde_json::from_slice(bytes).map_err(|error| AdapterError::Json(error.to_string()))?;
    reply.validate()?;
    Ok(reply)
}

pub fn decode_events_reply(bytes: &[u8]) -> Result<EventsReply, AdapterError> {
    check_reply_size(bytes)?;
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
/// changing the run filter, preventing pages from different queries mixing;
/// the reset also moves the request generation so a reply to the old filter
/// arriving late is rejected rather than folded as current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionListState {
    identity: StoreIdentity,
    run_id: Option<String>,
    sessions: Vec<SessionSummary>,
    next_cursor: Option<String>,
    has_more: bool,
    pages: usize,
    retained_bytes: usize,
    /// The generation of the query last handed out; 0 until one is minted.
    request_generation: u64,
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
            retained_bytes: 0,
            request_generation: 0,
        })
    }

    /// Mints the next page query. Minting again before the reply lands makes
    /// the earlier reply stale: only the latest ask is current.
    pub fn next_query(&mut self) -> Result<Option<FencedQuery>, AdapterError> {
        if self.pages != 0 && !self.has_more {
            return Ok(None);
        }
        let query = RunRecordsQuery::Sessions {
            run_id: self.run_id.clone(),
            after: self.next_cursor.clone(),
            limit: Some(DEFAULT_SESSION_PAGE),
        };
        encode_query(&query)?;
        self.request_generation = mint_generation();
        Ok(Some(FencedQuery {
            query,
            request_generation: self.request_generation,
        }))
    }

    /// Folds the reply bytes for `request_generation`. Every refusal leaves
    /// the state exactly as it was.
    pub fn fold(&mut self, request_generation: u64, bytes: &[u8]) -> Result<(), AdapterError> {
        check_generation(self.request_generation, request_generation)?;
        let page = decode_sessions_reply(bytes)?;
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
        check_budget(self.retained_bytes, bytes.len(), MAX_SESSION_LIST_BYTES)?;

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
        self.retained_bytes += bytes.len();
        Ok(())
    }

    /// Drops every page and moves the generation: a reply to any query minted
    /// before the reset is stale, even for the same filter.
    pub fn reset(&mut self, run_id: Option<String>) -> Result<(), AdapterError> {
        let mut replacement = Self::new(self.identity.clone(), run_id)?;
        replacement.request_generation = mint_generation();
        *self = replacement;
        Ok(())
    }

    pub fn identity(&self) -> &StoreIdentity {
        &self.identity
    }

    pub fn sessions(&self) -> &[SessionSummary] {
        &self.sessions
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryState {
    pub events: Vec<SessionEvent>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pages: usize,
    retained_bytes: usize,
    request_generation: u64,
}

impl HistoryState {
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentTailState {
    pub events: Vec<SessionEvent>,
    /// The last durable event cursor, reusable as the next tail `after`.
    pub high_water: Option<String>,
    /// This is display state only. It is never a continuation cursor.
    pub older_omitted: bool,
    retained_bytes: usize,
    request_generation: u64,
}

impl RecentTailState {
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
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

    /// Mints the next history page query; its generation is the history
    /// lane's own and never matches a tail generation.
    pub fn history_query(&mut self) -> Result<Option<FencedQuery>, AdapterError> {
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
        self.history.request_generation = mint_generation();
        Ok(Some(FencedQuery {
            query,
            request_generation: self.history.request_generation,
        }))
    }

    /// Tail polling is independent from historical pagination. `has_more`
    /// from a tail response is represented as `older_omitted`, never as a
    /// forward request condition.
    pub fn recent_tail_query(&mut self) -> Result<FencedQuery, AdapterError> {
        let query = RunRecordsQuery::Events {
            session_id: self.session_id.clone(),
            after: self.recent_tail.high_water.clone(),
            limit: Some(DEFAULT_EVENT_PAGE),
            tail: true,
        };
        encode_query(&query)?;
        self.recent_tail.request_generation = mint_generation();
        Ok(FencedQuery {
            query,
            request_generation: self.recent_tail.request_generation,
        })
    }

    pub fn fold_history(
        &mut self,
        request_generation: u64,
        bytes: &[u8],
    ) -> Result<(), AdapterError> {
        check_generation(self.history.request_generation, request_generation)?;
        let page = decode_events_reply(bytes)?;
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
        check_budget(self.history.retained_bytes, bytes.len(), MAX_HISTORY_BYTES)?;

        self.validate_append(&self.history.events, &page.events)?;
        self.history.events.extend(page.events);
        self.history.next_cursor = page.has_more.then_some(page.end_cursor);
        self.history.has_more = page.has_more;
        self.history.pages += 1;
        self.history.retained_bytes += bytes.len();
        Ok(())
    }

    /// The tail keeps the newest `MAX_RECENT_EVENTS` by count as before;
    /// bytes are measured on what would remain, and a page that would push
    /// that past the budget is refused without touching the state.
    pub fn fold_recent_tail(
        &mut self,
        request_generation: u64,
        bytes: &[u8],
    ) -> Result<(), AdapterError> {
        check_generation(self.recent_tail.request_generation, request_generation)?;
        let page = decode_events_reply(bytes)?;
        self.validate_event_reply(&page)?;
        self.validate_append(&self.recent_tail.events, &page.events)?;

        let mut events = self.recent_tail.events.clone();
        events.extend(page.events);
        let dropped = events.len().saturating_sub(MAX_RECENT_EVENTS);
        if dropped != 0 {
            events.drain(..dropped);
        }
        let retained = serde_json::to_vec(&events)
            .map_err(|error| AdapterError::Json(error.to_string()))?
            .len();
        check_budget(0, retained, MAX_RECENT_TAIL_BYTES)?;

        self.recent_tail.events = events;
        self.recent_tail.retained_bytes = retained;
        self.recent_tail.high_water = Some(page.end_cursor);
        self.recent_tail.older_omitted |= page.has_more || dropped != 0;
        Ok(())
    }

    /// Historical event payloads have no control authority by construction.
    pub const fn controls_enabled(&self) -> bool {
        false
    }

    /// Drops both lanes and moves both generations, so a reply in flight for
    /// either lane is stale after the reset.
    pub fn reset(&mut self) {
        self.history = HistoryState {
            request_generation: mint_generation(),
            ..HistoryState::default()
        };
        self.recent_tail = RecentTailState {
            request_generation: mint_generation(),
            ..RecentTailState::default()
        };
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

    fn bytes(reply: &impl Serialize) -> Vec<u8> {
        serde_json::to_vec(reply).unwrap()
    }

    fn session_query(state: &mut SessionListState) -> FencedQuery {
        state.next_query().unwrap().expect("a page to ask for")
    }

    #[test]
    fn session_pages_require_identity_and_reset_before_a_new_filter() {
        let id = identity("machine-a");
        let first = summary(&id, "session-a");
        let second = summary(&id, "session-b");
        let mut state = SessionListState::new(id.clone(), None).unwrap();
        let ask = session_query(&mut state);
        assert!(matches!(
            ask.query,
            RunRecordsQuery::Sessions { after: None, .. }
        ));
        state
            .fold(
                ask.request_generation,
                &bytes(&sessions_page(&id, vec![first], Some("opaque-next"), true)),
            )
            .unwrap();
        let ask = session_query(&mut state);
        assert!(matches!(
            ask.query,
            RunRecordsQuery::Sessions { after: Some(cursor), .. } if cursor == "opaque-next"
        ));
        state
            .fold(
                ask.request_generation,
                &bytes(&sessions_page(&id, vec![second], None, false)),
            )
            .unwrap();
        assert!(state.next_query().unwrap().is_none());
        assert_eq!(state.sessions().len(), 2);

        state.reset(Some(RUN_ID.into())).unwrap();
        assert!(state.sessions().is_empty());
        assert_eq!(state.retained_bytes(), 0);
        let ask = session_query(&mut state);
        assert!(matches!(
            ask.query,
            RunRecordsQuery::Sessions { run_id: Some(run_id), after: None, .. } if run_id == RUN_ID
        ));

        let wrong = identity("machine-b");
        assert_eq!(
            state
                .fold(
                    ask.request_generation,
                    &bytes(&sessions_page(&wrong, vec![], None, false))
                )
                .unwrap_err(),
            AdapterError::IdentityMismatch
        );
    }

    #[test]
    fn session_list_rejects_a_late_reply_after_a_b_a_filter_changes() {
        let id = identity("machine-a");
        let mut state = SessionListState::new(id.clone(), Some(RUN_ID.into())).unwrap();
        let first_a = session_query(&mut state);
        let reply_a = bytes(&sessions_page(
            &id,
            vec![summary(&id, "session-a")],
            None,
            false,
        ));

        state.reset(None).unwrap();
        let _b = session_query(&mut state);
        state.reset(Some(RUN_ID.into())).unwrap();
        let second_a = session_query(&mut state);
        assert_ne!(first_a.request_generation, second_a.request_generation);
        assert_eq!(
            first_a.query, second_a.query,
            "the same filter asks the same query"
        );

        assert_eq!(
            state
                .fold(first_a.request_generation, &reply_a)
                .unwrap_err(),
            AdapterError::StaleGeneration {
                current: second_a.request_generation,
                answered: first_a.request_generation,
            }
        );
        assert!(state.sessions().is_empty(), "a stale reply is not folded");
        state.fold(second_a.request_generation, &reply_a).unwrap();
        assert_eq!(state.sessions().len(), 1);
    }

    #[test]
    fn session_list_same_filter_reset_makes_the_in_flight_reply_stale() {
        let id = identity("machine-a");
        let mut state = SessionListState::new(id.clone(), None).unwrap();
        let ask = session_query(&mut state);
        let reply = bytes(&sessions_page(
            &id,
            vec![summary(&id, "session-a")],
            None,
            false,
        ));
        state.reset(None).unwrap();
        assert!(matches!(
            state.fold(ask.request_generation, &reply).unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        assert!(state.sessions().is_empty());
        // Nothing minted since the reset: no generation answers.
        assert!(matches!(
            state.fold(0, &reply).unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
    }

    #[test]
    fn session_list_refuses_a_generation_minted_by_another_state() {
        let id = identity("machine-a");
        let mut source = SessionListState::new(id.clone(), None).unwrap();
        let mut other = SessionListState::new(id.clone(), None).unwrap();
        let from_source = session_query(&mut source);
        let from_other = session_query(&mut other);
        assert_ne!(
            from_source.request_generation,
            from_other.request_generation
        );
        let reply = bytes(&sessions_page(&id, vec![], None, false));
        assert!(matches!(
            other
                .fold(from_source.request_generation, &reply)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        other.fold(from_other.request_generation, &reply).unwrap();
    }

    #[test]
    fn session_list_refuses_an_oversized_reply_before_decoding() {
        let id = identity("machine-a");
        let mut state = SessionListState::new(id.clone(), None).unwrap();
        let ask = session_query(&mut state);
        // Not even JSON: the size check must come first.
        let oversized = vec![b'x'; MAX_REPLY_BYTES + 1];
        assert_eq!(
            state.fold(ask.request_generation, &oversized).unwrap_err(),
            AdapterError::ReplyTooLarge {
                bytes: MAX_REPLY_BYTES + 1,
                limit: MAX_REPLY_BYTES,
            }
        );
        assert_eq!(
            decode_events_reply(&oversized).unwrap_err(),
            AdapterError::ReplyTooLarge {
                bytes: MAX_REPLY_BYTES + 1,
                limit: MAX_REPLY_BYTES,
            }
        );
        assert!(state.sessions().is_empty());
    }

    #[test]
    fn session_list_refuses_the_page_that_would_exceed_the_retained_budget() {
        let id = identity("machine-a");
        let mut state = SessionListState::new(id.clone(), None).unwrap();
        // Pad each summary so the byte budget trips well before the count caps.
        let mut page_index = 0;
        loop {
            let ask = session_query(&mut state);
            let mut item = summary(&id, &format!("session-{page_index}"));
            item.model = Some("m".repeat(MAX_REPLY_BYTES / 2));
            let cursor = format!("next-{page_index}");
            let page = bytes(&sessions_page(&id, vec![item], Some(&cursor), true));
            let before = state.retained_bytes();
            match state.fold(ask.request_generation, &page) {
                Ok(()) => {
                    assert_eq!(state.retained_bytes(), before + page.len());
                    page_index += 1;
                    assert!(page_index < MAX_SESSION_PAGES, "budget never tripped");
                }
                Err(AdapterError::RetainedBudgetExceeded { limit }) => {
                    assert_eq!(limit, MAX_SESSION_LIST_BYTES);
                    assert_eq!(state.retained_bytes(), before, "refusal keeps the state");
                    assert_eq!(state.sessions().len(), page_index);
                    break;
                }
                Err(other) => panic!("unexpected refusal {other}"),
            }
        }
        // A reset frees the budget; the caller was told that is the way out.
        state.reset(None).unwrap();
        assert_eq!(state.retained_bytes(), 0);
    }

    #[test]
    fn session_list_folds_the_session_for_run_reply_shape() {
        let id = identity("machine-a");
        let mut state = SessionListState::new(id.clone(), Some(RUN_ID.into())).unwrap();
        let ask = session_query(&mut state);
        // `session_for_run` answers with the same `sessions` envelope.
        let reply = json!({
            "kind": "sessions",
            "identity": {"machine_id": "machine-a", "network_id": "network-a"},
            "sessions": [{
                "session_id": "provider-session",
                "parent_session_id": null,
                "run_id": RUN_ID,
                "agent_id": "agent-a",
                "origin": {"kind": "delegation"},
                "requester": {"kind": "program", "account_id": 7},
                "model": null,
                "executor": null,
                "invocation_kind": "runs",
                "status": "completed",
                "started_at": "2026-09-20T09:00:00Z",
                "last_activity_at": null,
                "owner_account_id": null,
                "machine_id": "machine-a",
                "network_id": "network-a"
            }],
            "next_cursor": null,
            "has_more": false
        });
        state.fold(ask.request_generation, &bytes(&reply)).unwrap();
        assert_eq!(state.sessions()[0].status, SessionStatus::Completed);
        assert_eq!(state.sessions()[0].run_id.as_deref(), Some(RUN_ID));
        assert!(state.next_query().unwrap().is_none());
    }

    #[test]
    fn session_list_rejects_a_summary_mapped_to_another_run() {
        let id = identity("machine-a");
        let mut state = SessionListState::new(id.clone(), Some(RUN_ID.into())).unwrap();
        let ask = session_query(&mut state);
        let mut other_run = summary(&id, "session-a");
        other_run.run_id = Some("f".repeat(64));
        assert_eq!(
            state
                .fold(
                    ask.request_generation,
                    &bytes(&sessions_page(&id, vec![other_run], None, false))
                )
                .unwrap_err(),
            AdapterError::RunMismatch
        );
    }

    #[test]
    fn malformed_ids_and_limits_are_typed_query_refusals() {
        assert_eq!(
            encode_query(&RunRecordsQuery::Sessions {
                run_id: None,
                after: None,
                limit: Some(0),
            })
            .unwrap_err(),
            AdapterError::InvalidQuery("zero limit")
        );
        assert_eq!(
            encode_query(&RunRecordsQuery::Events {
                session_id: "provider-session".into(),
                after: None,
                limit: Some(MAX_EVENT_PAGE + 1),
                tail: false,
            })
            .unwrap_err(),
            AdapterError::InvalidQuery("limit too large")
        );
        assert_eq!(
            encode_query(&RunRecordsQuery::Events {
                session_id: "has space".into(),
                after: None,
                limit: None,
                tail: false,
            })
            .unwrap_err(),
            AdapterError::InvalidQuery("invalid session_id")
        );
        assert_eq!(
            encode_query(&RunRecordsQuery::Sessions {
                run_id: None,
                after: Some(String::new()),
                limit: None,
            })
            .unwrap_err(),
            AdapterError::InvalidQuery("empty cursor")
        );
        assert_eq!(
            SessionListState::new(identity("machine-a"), Some("short".into())).unwrap_err(),
            AdapterError::InvalidQuery("invalid run_id")
        );
    }

    #[test]
    fn unknown_fields_and_variants_are_json_refusals() {
        let id = identity("machine-a");
        let mut extra = serde_json::to_value(sessions_page(&id, vec![], None, false)).unwrap();
        extra["surprise"] = json!(1);
        assert!(matches!(
            decode_sessions_reply(&bytes(&extra)).unwrap_err(),
            AdapterError::Json(_)
        ));

        let mut unknown_status = serde_json::to_value(sessions_page(
            &id,
            vec![summary(&id, "session-a")],
            None,
            false,
        ))
        .unwrap();
        unknown_status["sessions"][0]["status"] = json!("paused");
        assert!(matches!(
            decode_sessions_reply(&bytes(&unknown_status)).unwrap_err(),
            AdapterError::Json(_)
        ));

        let wrong_kind = json!({
            "kind": "events",
            "identity": {"machine_id": "machine-a", "network_id": "network-a"},
            "sessions": [],
            "next_cursor": null,
            "has_more": false
        });
        assert_eq!(
            decode_sessions_reply(&bytes(&wrong_kind)).unwrap_err(),
            AdapterError::MalformedReply("wrong reply kind")
        );
    }

    #[test]
    fn a_host_refusal_stays_typed_and_never_reaches_a_decoder() {
        let refusal = host::Refusal::new("unauthorized", "only the requester");
        assert_eq!(
            reply_bytes(Err(refusal)).unwrap_err(),
            AdapterError::Refused {
                reason: "unauthorized".into(),
                sentence: "only the requester".into(),
            }
        );
        let id = identity("machine-a");
        let page = bytes(&sessions_page(&id, vec![], None, false));
        assert_eq!(reply_bytes(Ok(page.clone())).unwrap(), page);
    }

    #[test]
    fn history_and_tail_have_separate_cursors_and_never_grant_controls() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();

        let ask = state.history_query().unwrap().unwrap();
        state
            .fold_history(
                ask.request_generation,
                &bytes(&events_page(
                    &id,
                    "provider-session",
                    vec![event(1, "event-1", Some(RUN_ID)), event(2, "event-2", None)],
                    "history-cursor",
                    true,
                )),
            )
            .unwrap();
        let ask = state.history_query().unwrap().unwrap();
        assert!(matches!(
            ask.query,
            RunRecordsQuery::Events { after: Some(cursor), tail: false, .. } if cursor == "history-cursor"
        ));
        state
            .fold_history(
                ask.request_generation,
                &bytes(&events_page(
                    &id,
                    "provider-session",
                    vec![event(3, "event-3", Some(RUN_ID))],
                    "history-done",
                    false,
                )),
            )
            .unwrap();
        assert!(state.history_query().unwrap().is_none());

        let ask = state.recent_tail_query().unwrap();
        state
            .fold_recent_tail(
                ask.request_generation,
                &bytes(&events_page(
                    &id,
                    "provider-session",
                    vec![event(4, "event-4", Some(RUN_ID))],
                    "tail-high-water",
                    true,
                )),
            )
            .unwrap();
        assert!(state.recent_tail.older_omitted);
        let ask = state.recent_tail_query().unwrap();
        assert!(matches!(
            ask.query,
            RunRecordsQuery::Events { after: Some(cursor), tail: true, .. } if cursor == "tail-high-water"
        ));
        assert!(!state.controls_enabled());
    }

    #[test]
    fn history_and_tail_generations_are_independent() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();
        let history = state.history_query().unwrap().unwrap();
        let tail = state.recent_tail_query().unwrap();
        assert_ne!(history.request_generation, tail.request_generation);

        let page = bytes(&events_page(
            &id,
            "provider-session",
            vec![event(1, "event-1", None)],
            "cursor-1",
            false,
        ));
        assert!(matches!(
            state
                .fold_history(tail.request_generation, &page)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        assert!(matches!(
            state
                .fold_recent_tail(history.request_generation, &page)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        // Minting a new tail poll leaves the history ask current.
        let _later_tail = state.recent_tail_query().unwrap();
        state
            .fold_history(history.request_generation, &page)
            .unwrap();
        assert!(matches!(
            state
                .fold_recent_tail(tail.request_generation, &page)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        assert!(state.recent_tail.events.is_empty());
        assert!(!state.controls_enabled());
    }

    #[test]
    fn events_reset_makes_both_in_flight_replies_stale() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();
        let history = state.history_query().unwrap().unwrap();
        let tail = state.recent_tail_query().unwrap();
        let page = bytes(&events_page(
            &id,
            "provider-session",
            vec![],
            "cursor",
            false,
        ));
        state.reset();
        assert!(matches!(
            state
                .fold_history(history.request_generation, &page)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        assert!(matches!(
            state
                .fold_recent_tail(tail.request_generation, &page)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        assert!(state.recent_tail.high_water.is_none());
        assert_eq!(state.history.retained_bytes(), 0);
    }

    #[test]
    fn events_refuse_a_generation_minted_by_another_state_for_the_same_session() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut source = SessionEventsState::new(id.clone(), &summary).unwrap();
        let mut other = SessionEventsState::new(id.clone(), &summary).unwrap();
        let from_source = source.history_query().unwrap().unwrap();
        let from_other = other.history_query().unwrap().unwrap();
        let page = bytes(&events_page(
            &id,
            "provider-session",
            vec![],
            "cursor",
            false,
        ));
        assert!(matches!(
            other
                .fold_history(from_source.request_generation, &page)
                .unwrap_err(),
            AdapterError::StaleGeneration { .. }
        ));
        other
            .fold_history(from_other.request_generation, &page)
            .unwrap();
    }

    #[test]
    fn event_pages_reject_wrong_session_identity_and_explicit_run_mapping() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();
        let ask = state.history_query().unwrap().unwrap();
        let wrong_id = events_page(&id, "another-session", vec![], "cursor", false);
        assert_eq!(
            state
                .fold_history(ask.request_generation, &bytes(&wrong_id))
                .unwrap_err(),
            AdapterError::SessionMismatch
        );

        let wrong_run = event(1, "event-1", Some(&"f".repeat(64)));
        let page = events_page(&id, "provider-session", vec![wrong_run], "cursor", false);
        assert_eq!(
            state
                .fold_history(ask.request_generation, &bytes(&page))
                .unwrap_err(),
            AdapterError::RunMismatch
        );

        let wrong_store = events_page(
            &identity("machine-b"),
            "provider-session",
            vec![],
            "c",
            false,
        );
        assert_eq!(
            state
                .fold_history(ask.request_generation, &bytes(&wrong_store))
                .unwrap_err(),
            AdapterError::IdentityMismatch
        );
    }

    #[test]
    fn history_decodes_empty_and_ascending_pages_and_refuses_a_cursor_that_stalls() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();

        let ask = state.history_query().unwrap().unwrap();
        let ascending = events_page(
            &id,
            "provider-session",
            vec![event(1, "event-1", None), event(2, "event-2", None)],
            "cursor-2",
            true,
        );
        state
            .fold_history(ask.request_generation, &bytes(&ascending))
            .unwrap();
        assert_eq!(state.history.retained_bytes(), bytes(&ascending).len());

        let ask = state.history_query().unwrap().unwrap();
        let stalled = events_page(
            &id,
            "provider-session",
            vec![event(3, "event-3", None)],
            "cursor-2",
            true,
        );
        assert_eq!(
            state
                .fold_history(ask.request_generation, &bytes(&stalled))
                .unwrap_err(),
            AdapterError::CursorDidNotAdvance
        );

        let empty_done = events_page(&id, "provider-session", vec![], "cursor-end", false);
        state
            .fold_history(ask.request_generation, &bytes(&empty_done))
            .unwrap();
        assert!(!state.history.has_more);
        assert!(state.history_query().unwrap().is_none());

        let empty_more = json!({
            "kind": "events",
            "identity": {"machine_id": "machine-a", "network_id": "network-a"},
            "session_id": "provider-session",
            "events": [],
            "end_cursor": "cursor-x",
            "has_more": true
        });
        assert_eq!(
            decode_events_reply(&bytes(&empty_more)).unwrap_err(),
            AdapterError::MalformedReply("empty event page marked has_more")
        );

        let descending = events_page(
            &id,
            "provider-session",
            vec![event(2, "event-2", None), event(1, "event-1", None)],
            "cursor",
            false,
        );
        assert_eq!(
            decode_events_reply(&bytes(&descending)).unwrap_err(),
            AdapterError::MalformedReply("events are not ordered")
        );
    }

    #[test]
    fn history_refuses_the_page_that_would_exceed_the_retained_budget() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();
        let mut seq = 0;
        loop {
            let ask = state.history_query().unwrap().unwrap();
            seq += 1;
            let mut big = event(seq, &format!("event-{seq}"), None);
            big.payload = json!({"blob": "b".repeat(MAX_REPLY_BYTES / 2)});
            let page = bytes(&events_page(
                &id,
                "provider-session",
                vec![big],
                &format!("c{seq}"),
                true,
            ));
            let before = state.history.retained_bytes();
            match state.fold_history(ask.request_generation, &page) {
                Ok(()) => assert!(seq < MAX_HISTORY_PAGES as u64, "budget never tripped"),
                Err(AdapterError::RetainedBudgetExceeded { limit }) => {
                    assert_eq!(limit, MAX_HISTORY_BYTES);
                    assert_eq!(state.history.retained_bytes(), before);
                    assert_eq!(state.history.events.len() as u64, seq - 1);
                    break;
                }
                Err(other) => panic!("unexpected refusal {other}"),
            }
        }
    }

    #[test]
    fn tail_keeps_has_more_display_only_and_refuses_a_page_over_its_budget() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();

        let ask = state.recent_tail_query().unwrap();
        let quiet = events_page(
            &id,
            "provider-session",
            vec![event(1, "event-1", None)],
            "hw-1",
            false,
        );
        state
            .fold_recent_tail(ask.request_generation, &bytes(&quiet))
            .unwrap();
        assert!(!state.recent_tail.older_omitted);
        assert_eq!(state.recent_tail.high_water.as_deref(), Some("hw-1"));
        assert_eq!(
            state.recent_tail.retained_bytes(),
            bytes(&state.recent_tail.events).len()
        );

        let ask = state.recent_tail_query().unwrap();
        let truncated = events_page(
            &id,
            "provider-session",
            vec![event(2, "event-2", None)],
            "hw-2",
            true,
        );
        state
            .fold_recent_tail(ask.request_generation, &bytes(&truncated))
            .unwrap();
        assert!(
            state.recent_tail.older_omitted,
            "has_more is shown, never followed"
        );
        assert!(matches!(
            state.recent_tail_query().unwrap().query,
            RunRecordsQuery::Events { after: Some(cursor), tail: true, .. } if cursor == "hw-2"
        ));
        assert!(
            state.history_query().unwrap().is_some(),
            "the history lane is untouched"
        );
        assert!(state.history.events.is_empty());

        // Two heavy polls: the first fits, the second would push the retained
        // tail past its budget and is refused whole.
        let heavy = |seq: u64| {
            let mut heavy = event(seq, &format!("event-{seq}"), None);
            heavy.payload = json!({"blob": "b".repeat(MAX_RECENT_TAIL_BYTES / 2 + 1024)});
            heavy
        };
        let ask = state.recent_tail_query().unwrap();
        let first = events_page(&id, "provider-session", vec![heavy(3)], "hw-3", false);
        state
            .fold_recent_tail(ask.request_generation, &bytes(&first))
            .unwrap();
        let ask = state.recent_tail_query().unwrap();
        let over = events_page(&id, "provider-session", vec![heavy(4)], "hw-4", false);
        assert_eq!(
            state
                .fold_recent_tail(ask.request_generation, &bytes(&over))
                .unwrap_err(),
            AdapterError::RetainedBudgetExceeded {
                limit: MAX_RECENT_TAIL_BYTES
            }
        );
        assert_eq!(state.recent_tail.events.len(), 3, "refusal keeps the tail");
        assert_eq!(state.recent_tail.high_water.as_deref(), Some("hw-3"));
        assert!(!state.controls_enabled());
    }

    #[test]
    fn tail_count_eviction_flags_older_omitted_and_stays_within_budget() {
        let id = identity("machine-a");
        let summary = summary(&id, "provider-session");
        let mut state = SessionEventsState::new(id.clone(), &summary).unwrap();
        let ask = state.recent_tail_query().unwrap();
        let events: Vec<_> = (1..=MAX_RECENT_EVENTS as u64 + 1)
            .map(|seq| event(seq, &format!("event-{seq}"), None))
            .collect();
        let page = events_page(&id, "provider-session", events, "hw", false);
        state
            .fold_recent_tail(ask.request_generation, &bytes(&page))
            .unwrap();
        assert_eq!(state.recent_tail.events.len(), MAX_RECENT_EVENTS);
        assert_eq!(state.recent_tail.events[0].seq, 2);
        assert!(state.recent_tail.older_omitted);
        assert!(state.recent_tail.retained_bytes() <= MAX_RECENT_TAIL_BYTES);
    }
}
