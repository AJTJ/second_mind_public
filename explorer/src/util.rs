/// Resolve the intake engine binary path.
/// Checks INTAKE_ENGINE_BIN env var first, then falls back to the sibling
/// binary in the project build tree.
pub fn intake_engine_bin() -> String {
    if let Ok(bin) = std::env::var("INTAKE_ENGINE_BIN") {
        return bin;
    }

    // Default: sibling binary relative to current exe
    // explorer/target/release/explorer → intake-engine/target/release/intake-engine
    let path = std::env::current_exe().ok().and_then(|mut p| {
        p.pop(); // bin dir (release/)
        p.pop(); // target/
        p.pop(); // explorer/
        p.push("intake-engine");
        p.push("target");
        p.push("release");
        p.push("intake-engine");
        if p.exists() { Some(p) } else { None }
    });

    match path {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            tracing::warn!("intake-engine binary not found. Set INTAKE_ENGINE_BIN env var.");
            "intake-engine".to_string() // fall back to PATH lookup
        }
    }
}
