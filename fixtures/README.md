# View-local producer fixtures

These fixtures are public synthetic JSON produced from the authoritative SDK
module types at locked revision `b66f47f1f4b0c869786ce195e382f2e83fd15277`
(the `Cargo.lock` `ducktape-sdk` revision). They were generated on 2026-09-20
with isolated stdin-only `rustc` probes against already-built SDK rlibs; the
probes were outside this workspace's production dependency graph and are not
part of the build. JSON whitespace is normalized, so tests compare parsed JSON
semantics rather than a local type round-trip.

| Consumer path | Producer input | Fixture |
| --- | --- | --- |
| Canvas board read/write | `boards-wire::Reply::Board` and `Operation::Edit` | `canvas/tests/fixtures/boards/` |
| Composer rich text | `chat-message::parse_message` | `support/composer/tests/fixtures/message-rich.json` |
| File address support | `files-wire::FileAddress` | `support/files/tests/fixtures/file-address.json` |
| Pages address consumers | `duck_address::pages::PageAddress` (the producer type formerly re-exported by `pages-wire`) | `pages/tests/fixtures/page-address.json` |
| Agents registration | `runs-wire::model_program`, `agent-wire::AgentReply::Provision` | `agents/tests/fixtures/` |

The fixtures are evidence of producer parity at that revision, not a promise
that a future producer change is compatible. A producer change should update
the authoritative source and the affected consumer fixture together.
