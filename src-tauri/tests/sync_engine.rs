// NOTE: The end-to-end engine test (real server + engine) requires a Tauri
// runtime handle to emit events. The mock-runtime approach (`tauri::test`)
// fails to load on Windows with STATUS_ENTRYPOINT_NOT_FOUND (WebView2 loader
// mismatch), so it is not used. The engine's stop path is instead covered by
// unit tests in `sync/engine.rs` and manual verification in the running app.

