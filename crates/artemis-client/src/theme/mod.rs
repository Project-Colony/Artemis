use iced::color;
use iced::Theme;

/// Nerd Font icon constants (JetBrains Mono Nerd Font glyphs).
#[allow(dead_code)]
pub mod icons {
    // ── Channel types ──
    /// Text channel icon (nf-oct-hash)
    pub const CHANNEL_TEXT: &str = "\u{f292}";
    /// Voice channel icon (nf-fa-volume_up)
    pub const CHANNEL_VOICE: &str = "\u{f028}";

    // ── Navigation ──
    /// Home icon (nf-fa-home)
    pub const HOME: &str = "\u{f015}";
    /// Server icon (nf-cod-server)
    pub const SERVER: &str = "\u{f473}";
    /// People / members icon (nf-fa-users)
    pub const MEMBERS: &str = "\u{f0c0}";
    /// Settings gear icon (nf-fa-cog)
    pub const SETTINGS: &str = "\u{f013}";
    /// Plus / add icon (nf-fa-plus)
    pub const PLUS: &str = "\u{f067}";

    // ── Message actions ──
    /// Reply icon (nf-fa-reply)
    pub const REPLY: &str = "\u{f112}";
    /// Pin icon (nf-fa-thumb_tack)
    pub const PIN: &str = "\u{f08d}";
    /// Unpin icon (same as pin but contextually different)
    pub const UNPIN: &str = "\u{f08d}";
    /// Send / paper plane icon (nf-fa-paper_plane)
    pub const SEND: &str = "\u{f1d8}";
    /// Edit icon (nf-fa-pencil)
    pub const EDIT: &str = "\u{f040}";
    /// Delete / trash icon (nf-fa-trash)
    pub const DELETE: &str = "\u{f1f8}";
    /// Close / X icon (nf-fa-times)
    pub const CLOSE: &str = "\u{f00d}";

    // ── Search ──
    /// Search / magnifying glass (nf-fa-search)
    pub const SEARCH: &str = "\u{f002}";

    // ── Status ──
    /// Online circle (nf-fa-circle)
    pub const STATUS_ONLINE: &str = "\u{f111}";
    /// Idle moon (nf-fa-moon_o)
    pub const STATUS_IDLE: &str = "\u{f186}";
    /// Do not disturb minus circle (nf-fa-minus_circle)
    pub const STATUS_DND: &str = "\u{f056}";
    /// Offline empty circle (nf-fa-circle_o)
    pub const STATUS_OFFLINE: &str = "\u{f10c}";

    // ── Friends / Social ──
    /// Friend / user icon (nf-fa-user)
    pub const USER: &str = "\u{f007}";
    /// Add friend (nf-fa-user_plus)
    pub const USER_PLUS: &str = "\u{f234}";
    /// Friend request pending (nf-fa-clock_o)
    pub const PENDING: &str = "\u{f017}";
    /// Accept / check (nf-fa-check)
    pub const CHECK: &str = "\u{f00c}";
    /// Decline / X (nf-fa-times)
    pub const DECLINE: &str = "\u{f00d}";

    // ── Messages ──
    /// Chat / comment icon (nf-fa-comment)
    pub const CHAT: &str = "\u{f075}";
    /// Reply chain / thread (nf-cod-git_merge)
    pub const THREAD: &str = "\u{ea4c}";
    /// Attachment / paperclip (nf-fa-paperclip)
    pub const ATTACHMENT: &str = "\u{f0c6}";

    // ── Misc ──
    /// Lock (nf-fa-lock)
    pub const LOCK: &str = "\u{f023}";
    /// Globe (nf-fa-globe)
    pub const GLOBE: &str = "\u{f0ac}";
    /// Star (nf-fa-star)
    pub const STAR: &str = "\u{f005}";
    /// Bell / notification (nf-fa-bell)
    pub const BELL: &str = "\u{f0f3}";
    /// Emoji / smile (nf-fa-smile_o)
    pub const EMOJI: &str = "\u{f118}";
    /// Arrow up / load older (nf-fa-arrow_up)
    pub const ARROW_UP: &str = "\u{f062}";
    /// Category collapse arrow (nf-fa-chevron_down)
    pub const CHEVRON_DOWN: &str = "\u{f078}";
    /// Category expand arrow (nf-fa-chevron_right)
    pub const CHEVRON_RIGHT: &str = "\u{f054}";
    /// Separator bar (nf-pl-left_hard_divider)
    pub const DIVIDER: &str = "\u{e0b0}";
}

/// Color constants for Artemis dark theme
pub mod colors {
    use iced::Color;

    pub const BG_DARKEST: Color = Color::from_rgb(
        0x1E as f32 / 255.0,
        0x1E as f32 / 255.0,
        0x2E as f32 / 255.0,
    );
    pub const BG_DARK: Color = Color::from_rgb(
        0x2B as f32 / 255.0,
        0x2D as f32 / 255.0,
        0x42 as f32 / 255.0,
    );
    pub const BG_MEDIUM: Color = Color::from_rgb(
        0x30 as f32 / 255.0,
        0x32 as f32 / 255.0,
        0x48 as f32 / 255.0,
    );
    pub const BG_INPUT: Color = Color::from_rgb(
        0x38 as f32 / 255.0,
        0x3A as f32 / 255.0,
        0x5C as f32 / 255.0,
    );
    pub const BG_HOVER: Color = Color::from_rgb(
        0x3D as f32 / 255.0,
        0x3F as f32 / 255.0,
        0x63 as f32 / 255.0,
    );
    pub const BG_ACTIVE: Color = Color::from_rgb(
        0x4A as f32 / 255.0,
        0x4C as f32 / 255.0,
        0x70 as f32 / 255.0,
    );

    pub const TEXT_PRIMARY: Color = Color::WHITE;
    pub const TEXT_MUTED: Color = Color::from_rgb(
        0xA0 as f32 / 255.0,
        0xA0 as f32 / 255.0,
        0xB8 as f32 / 255.0,
    );
    pub const TEXT_TIMESTAMP: Color = Color::from_rgb(
        0x6E as f32 / 255.0,
        0x6E as f32 / 255.0,
        0x8A as f32 / 255.0,
    );

    pub const ROLE_FOUNDER: Color = Color::from_rgb(
        0x4A as f32 / 255.0,
        0xDE as f32 / 255.0,
        0x80 as f32 / 255.0,
    );
    pub const ROLE_MODERATOR: Color = Color::from_rgb(
        0x60 as f32 / 255.0,
        0xA5 as f32 / 255.0,
        0xFA as f32 / 255.0,
    );

    pub const STATUS_ONLINE: Color = Color::from_rgb(
        0x22 as f32 / 255.0,
        0xC5 as f32 / 255.0,
        0x5E as f32 / 255.0,
    );
    pub const STATUS_IDLE: Color = Color::from_rgb(
        0xF5 as f32 / 255.0,
        0x9E as f32 / 255.0,
        0x0B as f32 / 255.0,
    );

    pub const ACCENT: Color = Color::from_rgb(
        0x7C as f32 / 255.0,
        0x3A as f32 / 255.0,
        0xED as f32 / 255.0,
    );
}

pub fn artemis_theme() -> Theme {
    Theme::custom(
        "Artemis Dark".to_string(),
        iced::theme::Palette {
            background: colors::BG_MEDIUM,
            text: colors::TEXT_PRIMARY,
            primary: colors::ACCENT,
            success: colors::STATUS_ONLINE,
            warning: colors::STATUS_IDLE,
            danger: color!(0xEF, 0x44, 0x44),
        },
    )
}
