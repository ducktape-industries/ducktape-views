# ducktape-views

Rust-authored WASM views for Ducktape's desktop app. Each crate in this
workspace compiles to a `<id>_view.wasm` component: a renderer-independent
tree the app's GPUI shell draws and drives, never a UI toolkit compiled for
the browser. `ops/build-views.sh` builds the set through
`wasm32-unknown-unknown`, and `ops/views-repro-check.sh` proves two clean
checkouts produce byte-identical components with no builder-local path baked
into a symbol hash.

`support/guest` is the renderer-independent execution and host-protocol
crate every view links; `support/composer` and `support/files` are the
shared write-lanes a view commits a chat message or a file through. Every
other crate at the workspace root — `agents`, `call`, `canvas`, `chat`,
`explorer`, `files`, `forge`, `governance`, `home`, `inbox`, `members`,
`node`, `pages`, `palette`, `settings` — is one view.

## Repo DAG

```
ducktape-sdk
├── ducktape          (the node/kernel; also depends on ducktape-sdk)
│   └── ducktape-app  (the desktop app; depends on ducktape and ducktape-sdk)
├── ducktape-modules   (consensus modules; depends on ducktape-sdk)
└── ducktape-views     (this repo; depends on ducktape-sdk)
```

`ducktape-sdk` is the only thing a view or a module may depend on: the
module contract, the host↔view wire (`view-wire`), the shared design system
(`design`), and the per-module wire crates (`chat-message`, `boards-wire`,
`runs-wire`, `agent-wire`, `duckfs-core`, …) that describe a module's format
without linking the module itself. A view never depends on `ducktape` or on
a module's implementation crate — only on the wire and SDK types published
from `ducktape-sdk`, pinned by git revision like every other cross-repo
dependency in this DAG.

## How this is consumed

The desktop app (`ducktape-app`) loads each built `<id>_view.wasm` as a
file at runtime, the same way the node loads a module component: a view
ships and pins independently of the app binary that runs it. Building this
repo produces the `.wasm` bundle; getting it onto a running app is an
install or hydration step, never a compile step into the app.
