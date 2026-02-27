pub fn mcp_server_command() -> String {
    if let Ok(cmd) = std::env::var("DARLING_MCP_SERVER_COMMAND") {
        if !cmd.trim().is_empty() {
            return cmd;
        }
    }

    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo_toml = manifest_dir.join("Cargo.toml");
    format!(
        "cargo run --manifest-path {} --bin darling_mcp --quiet",
        cargo_toml.to_string_lossy()
    )
}
