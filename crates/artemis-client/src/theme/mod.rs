use iced::color;
use iced::Theme;

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
            danger: color!(0xEF, 0x44, 0x44),
        },
    )
}
