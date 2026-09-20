# View-local producer fixtures

These fixtures are public synthetic JSON produced from authoritative SDK
producer types at locked revision `8c764093d5974b4328c0b90de157d72597540020`
(the `Cargo.lock` `ducktape-sdk` revision). The exact inputs, producer types,
and b66 comparison are in `fixtures/provenance.json`. The historical generator
is preserved at repository revision `f150990d0943e457adb7765abb70f0a9434e2b6e`;
it is no longer a dependency of this repository.
JSON whitespace is not part of the contract, so tests compare parsed JSON
semantics rather than a local type round-trip.

| Consumer path | Producer input | Fixture |
| --- | --- | --- |
| Canvas board read/write | `boards-wire::Reply::Board` and `Operation::Edit` | `canvas/tests/fixtures/boards/` |
| Composer rich text | `chat-message::parse_message` | `support/composer/tests/fixtures/message-rich.json` |
| File address support | `files-wire::FileAddress` | `support/files/tests/fixtures/file-address.json` |
| Pages address consumers | `duck_address::pages::PageAddress` (the producer type formerly re-exported by `pages-wire`) | `pages/tests/fixtures/page-address.json` |
| Agents registration | `runs-wire::model_program`, `agent-wire::AgentReply::Provision`, `agent-wire::AgentMsg::Provision`, `runs-wire::RunsMsg::ConfigureModel` | `agents/tests/fixtures/` |

The fixtures are evidence of producer parity at that revision, not a promise
that a future producer change is compatible. A producer change should update
the authoritative source and the affected consumer fixture together. The
agent request fixtures include the app's `op.submit` target envelope around
the producer-encoded module payload so tests assert the complete emitted
request.
