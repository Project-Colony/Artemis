//! Size limits shared by the client and the relay. The client keeps what the
//! user types within them, and the relay refuses anything over them before it
//! touches the database.

/// The most characters a message may have. A message at the cap fits the
/// relay's 64 KiB frame cap, as a direct message too once encrypted.
pub const MAX_MESSAGE_CHARS: usize = 4000;

/// The longest direct message ciphertext, in bytes: a message of
/// MAX_MESSAGE_CHARS characters of 4 UTF-8 bytes each, behind its 12-byte
/// nonce and followed by its 16-byte tag, in padded base64. Base64 is ASCII,
/// so this is also its length in characters.
pub const MAX_DM_CIPHERTEXT_BYTES: usize = 4 * (12 + 4 * MAX_MESSAGE_CHARS + 16).div_ceil(3);

/// The most characters in a server, category, channel, emoji or display
/// name, and in a reaction. The database columns are VARCHAR(64).
pub const MAX_NAME_CHARS: usize = 64;

/// The most characters in a channel topic or an image URL.
pub const MAX_TOPIC_CHARS: usize = 1024;

/// The most characters in a custom status. The column is VARCHAR(128).
pub const MAX_STATUS_CHARS: usize = 128;

/// The most users one message can mention. Further names are not counted.
pub const MAX_MENTIONS: usize = 20;

/// Why a text was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// More characters than the limit.
    TooLong,
    /// A NUL character, which Postgres text cannot hold.
    Nul,
}

/// Checks `text` against a limit of `max_chars` characters, counted as Rust
/// chars, which is how Postgres counts VARCHAR(n).
pub fn check_text(text: &str, max_chars: usize) -> Result<(), TextError> {
    if text.contains('\0') {
        return Err(TextError::Nul);
    }
    // Counting stops one past the limit, however long the text is.
    if text.chars().nth(max_chars).is_some() {
        return Err(TextError::TooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_count_characters_not_bytes() {
        // Each of these is four bytes.
        let at_cap = "\u{1F600}".repeat(MAX_MESSAGE_CHARS);
        assert_eq!(check_text(&at_cap, MAX_MESSAGE_CHARS), Ok(()));
        let over = "\u{1F600}".repeat(MAX_MESSAGE_CHARS + 1);
        assert_eq!(
            check_text(&over, MAX_MESSAGE_CHARS),
            Err(TextError::TooLong)
        );
        assert_eq!(check_text("", MAX_NAME_CHARS), Ok(()));
    }

    #[test]
    fn nul_is_refused_at_any_length() {
        assert_eq!(check_text("a\0b", MAX_MESSAGE_CHARS), Err(TextError::Nul));
        assert_eq!(check_text("\0", MAX_NAME_CHARS), Err(TextError::Nul));
    }
}
