//! Wire codec for the tool side of SDK protocol v1. No browser deps, so the
//! rules are unit-tested natively; the `web` transport adds the
//! `event.source === window.parent` check, which needs a live `Window`.

use console_protocol::{MessageError, ShellToTool, ToolToShell};

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("message from untrusted origin {0:?}")]
    UntrustedOrigin(String),
    #[error(transparent)]
    Message(#[from] MessageError),
}

/// Exact-match origin pin. `console_origin` comes from the gateway's
/// `/_console/context`, never from `document.referrer` or the URL.
pub fn check_origin(origin: &str, console_origin: &str) -> Result<(), SdkError> {
    // An empty pin (context not loaded yet) or the opaque "null" origin never matches.
    if console_origin.is_empty() || origin == "null" || origin != console_origin {
        return Err(SdkError::UntrustedOrigin(origin.into()));
    }
    Ok(())
}

/// Decode a message from the shell: origin pin, 16 KiB cap, then lenient
/// parse. `Ok(None)` means a well-formed message of a type this SDK does
/// not know (from a newer shell); callers ignore it.
pub fn decode(
    origin: &str,
    console_origin: &str,
    data: &str,
) -> Result<Option<ShellToTool>, SdkError> {
    check_origin(origin, console_origin)?;
    Ok(ShellToTool::from_json(data)?)
}

/// Serialize a message for `window.parent.postMessage(s, console_origin)`.
pub fn encode(msg: &ToolToShell) -> Result<String, SdkError> {
    Ok(msg.to_json()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_protocol::MAX_MESSAGE_BYTES;

    const CONSOLE: &str = "https://console.local";

    #[test]
    fn rejects_foreign_origin() {
        let data = r#"{"type":"ThemeChanged","theme":"dark"}"#;
        for bad in [
            "https://evil.example",
            "null",
            "",
            "https://console.local:443x",
        ] {
            assert!(
                matches!(
                    decode(bad, CONSOLE, data),
                    Err(SdkError::UntrustedOrigin(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            decode("null", "null", data),
            Err(SdkError::UntrustedOrigin(_))
        ));
        assert!(matches!(
            decode("", "", data),
            Err(SdkError::UntrustedOrigin(_))
        ));
        assert!(matches!(
            decode(CONSOLE, CONSOLE, data),
            Ok(Some(ShellToTool::ThemeChanged { .. }))
        ));
    }

    #[test]
    fn unknown_type_is_none_and_size_capped() {
        assert!(matches!(
            decode(CONSOLE, CONSOLE, r#"{"type":"Later","x":1}"#),
            Ok(None)
        ));
        let big = format!(
            r#"{{"type":"Navigate","path":"/{}"}}"#,
            "a".repeat(MAX_MESSAGE_BYTES)
        );
        assert!(matches!(
            decode(CONSOLE, CONSOLE, &big),
            Err(SdkError::Message(MessageError::TooLarge(_)))
        ));
    }

    #[test]
    fn encode_shape() {
        let s = encode(&ToolToShell::FocusShell { reverse: false }).unwrap();
        assert_eq!(s, r#"{"type":"FocusShell","reverse":false}"#);
    }
}
