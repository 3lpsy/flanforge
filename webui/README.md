# Web UI

The Dioxus single-page app `flanforged` serves at `/ui`. It is compiled to
wasm, written into `webui/dist/`, and embedded into the daemon at build time;
a UI-less checkout still builds, and `/ui` then answers 503.


# Building 

## Crates

- `app` — the SPA: routes, pages, and session state.
- `api-client` — thin fetch and SSE wrappers over `/api/v1`.
- `components` — shared tables, widgets, and formatting.

## Building

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <Cargo.lock's wasm-bindgen>
just ui-build   # wasm build + bindgen + static files -> webui/dist/
```

Rebuild the daemon afterwards to refresh the embed. For iteration,
`just ui-dev` runs the daemon with `webui.dev_dist_dir` pointing at
`webui/dist/`, so assets are read from disk and the daemon is not relinked.
`just ui-check` and `just ui-clippy` cover the wasm targets.

The UI talks only to `/api/v1`; authentication, tiers, and serving live in
`crates/flanforge-webui-routes`.
