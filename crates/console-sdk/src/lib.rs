//! Tool-side SDK. M0 provides encode/decode and origin checking; the
//! browser `postMessage` transport and Yew hooks land in M4.

pub use console_protocol::{PROTOCOL_VERSION, ShellToTool, ToolToShell};

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("message from untrusted origin {0:?}")]
    UntrustedOrigin(String),
    #[error("malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
}

/// Decode a message from the shell, rejecting anything not from `console_origin`.
pub fn decode(origin: &str, console_origin: &str, data: &str) -> Result<ShellToTool, SdkError> {
    if origin != console_origin {
        return Err(SdkError::UntrustedOrigin(origin.into()));
    }
    Ok(serde_json::from_str(data)?)
}

pub fn encode(msg: &ToolToShell) -> String {
    serde_json::to_string(msg).expect("ToolToShell is always serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_foreign_origin() {
        let data = r#"{"type":"ThemeChanged","theme":"dark"}"#;
        assert!(matches!(
            decode("https://evil.example", "https://console.local", data),
            Err(SdkError::UntrustedOrigin(_))
        ));
        assert!(decode("https://console.local", "https://console.local", data).is_ok());
    }
}
