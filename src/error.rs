use anyhow::Error;

/// Render an anyhow error chain into a user-friendly message.
///
/// HTTP-layer hints (e.g. the Feilian/VPN note for 403s) are produced at the
/// source — see `http::check_status` — so this only flattens the chain.
pub fn render(err: &Error) -> String {
    let chain: Vec<String> = err.chain().map(|e| e.to_string()).collect();
    if chain.len() == 1 {
        chain[0].clone()
    } else {
        chain.join("\n  caused by: ")
    }
}
