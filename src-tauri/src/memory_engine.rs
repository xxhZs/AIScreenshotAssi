// memory_engine.rs — stub implementation.
//
// Will be replaced with a SQLite-backed semantic memory store in a later
// iteration.  The function signature is intentionally kept stable so that
// call-sites in main / command handlers don't need to change.

/// Query the memory store for context relevant to `input`.
///
/// Currently returns a hard-coded placeholder so the rest of the pipeline
/// can be wired up before the storage layer is built.
pub fn query_memory(_input: &str) -> String {
    "[memory_engine] stub — no persistent memory yet".to_string()
}
