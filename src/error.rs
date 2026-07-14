use anyhow::Error;

/// Render an anyhow error chain into a user-friendly message.
///
/// Special-cases HTTP 403 responses with a prominent Feilian hint, since the
/// Loki endpoints are only reachable via Feilian/VPN.
pub fn render(err: &Error) -> String {
    let chain: Vec<String> = err.chain().map(|e| e.to_string()).collect();

    // Check for 403 anywhere in the error chain.
    for msg in &chain {
        if msg.contains("403") {
            return format!(
                "HTTP 403 Forbidden\n\
                 This endpoint is only reachable via Feilian/VPN.\n\
                 Please connect to Feilian first, then retry.\n\
                 \n\
                 {}",
                chain.last().unwrap_or(msg)
            );
        }
    }

    // Default: print the full error chain.
    if chain.len() == 1 {
        chain[0].clone()
    } else {
        chain.join("\n  caused by: ")
    }
}
