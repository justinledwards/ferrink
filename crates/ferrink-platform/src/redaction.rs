pub const REDACTION_POLICY: &str = "ferrink-wp1-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedSerial {
    pub prefix: String,
    pub display: String,
}

/// Keep only the model-identifying prefix from a Kindle serial.
///
/// Newer `G000...` serials use the three-character prefix at positions 4–6
/// (for example `0AA`). Older serials use their first four characters.
pub fn redact_serial(input: &str) -> Option<RedactedSerial> {
    let normalized: String = input
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_uppercase())
        .collect();
    if normalized.len() < 4 {
        return None;
    }

    let (prefix, visible) = if normalized.starts_with("G000") && normalized.len() >= 6 {
        (normalized[3..6].to_owned(), normalized[..6].to_owned())
    } else {
        (normalized[..4].to_owned(), normalized[..4].to_owned())
    };
    Some(RedactedSerial {
        prefix,
        display: format!("{visible}…REDACTED"),
    })
}

/// Sanitize a short diagnostic value and suppress likely secrets or full
/// device identifiers. Collection code still uses strict allowlists first.
pub fn redact_text(input: &str) -> String {
    let collapsed = input
        .chars()
        .map(|character| {
            if character.is_control() && !character.is_ascii_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let mut output = Vec::new();
    for token in collapsed.split_whitespace().take(64) {
        let lower = token.to_ascii_lowercase();
        if [
            "token",
            "password",
            "passwd",
            "secret",
            "cookie",
            "private_key",
            "ssid",
            "account",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            output.push("<redacted>".to_owned());
            continue;
        }
        let alphanumeric = token
            .trim_matches(|character: char| !character.is_ascii_alphanumeric())
            .to_ascii_uppercase();
        if alphanumeric.len() >= 12
            && (alphanumeric.starts_with('G') || alphanumeric.starts_with('B'))
            && alphanumeric
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        {
            output.push(
                redact_serial(&alphanumeric)
                    .map(|serial| serial.display)
                    .unwrap_or_else(|| "<redacted-identifier>".to_owned()),
            );
        } else {
            output.push(token.chars().take(128).collect());
        }
    }
    output.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{redact_serial, redact_text};

    #[test]
    fn newer_serial_keeps_only_documented_prefix() {
        let serial = redact_serial("G000AA0123456789").unwrap();
        assert_eq!(serial.prefix, "0AA");
        assert_eq!(serial.display, "G000AA…REDACTED");
        assert!(!serial.display.contains("123456789"));
    }

    #[test]
    fn older_serial_keeps_four_character_prefix() {
        let serial = redact_serial("DEMO123456789012").unwrap();
        assert_eq!(serial.prefix, "DEMO");
        assert_eq!(serial.display, "DEMO…REDACTED");
    }

    #[test]
    fn diagnostic_text_removes_secret_markers_and_full_serials() {
        let output = redact_text("token=abc G000AA0123456789 safe");
        assert!(!output.contains("abc"));
        assert!(!output.contains("123456789"));
        assert!(output.contains("0AA"));
    }
}
