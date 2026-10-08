use artemis_core::models::{
    channel::{Category, Channel, ChannelType},
    message::Message,
    server::Server,
    user::{MemberRole, ServerMember, User, UserStatus},
};
use chrono::Utc;
use uuid::Uuid;

/// Generate sample data for UI development and testing.
pub fn sample_data() -> (Vec<Server>, Vec<Message>, Vec<ServerMember>) {
    let general_id = Uuid::new_v4();
    let dev_id = Uuid::new_v4();
    let offtopic_id = Uuid::new_v4();
    let music_id = Uuid::new_v4();
    let art_id = Uuid::new_v4();

    let servers = vec![
        Server {
            id: Uuid::new_v4(),
            name: "Artemis".to_string(),
            icon_url: None,
            owner_id: Uuid::new_v4(),
            categories: vec![
                Category {
                    id: Uuid::new_v4(),
                    name: "DISCUSSION".to_string(),
                    collapsed: false,
                    channels: vec![
                        Channel {
                            id: general_id,
                            name: "General".to_string(),
                            channel_type: ChannelType::Text,
                            topic: Some("Welcome to Artemis! General chat goes here.".to_string()),
                            icon: Some("\u{f292}".to_string()),
                        },
                        Channel {
                            id: dev_id,
                            name: "Development".to_string(),
                            channel_type: ChannelType::Text,
                            topic: Some("Artemis development discussion".to_string()),
                            icon: Some("\u{f292}".to_string()),
                        },
                        Channel {
                            id: Uuid::new_v4(),
                            name: "Suggestions".to_string(),
                            channel_type: ChannelType::Text,
                            topic: None,
                            icon: Some("\u{f292}".to_string()),
                        },
                    ],
                },
                Category {
                    id: Uuid::new_v4(),
                    name: "OFF TOPIC".to_string(),
                    collapsed: false,
                    channels: vec![
                        Channel {
                            id: offtopic_id,
                            name: "Off Topic".to_string(),
                            channel_type: ChannelType::Text,
                            topic: None,
                            icon: Some("\u{f292}".to_string()),
                        },
                        Channel {
                            id: music_id,
                            name: "Music".to_string(),
                            channel_type: ChannelType::Text,
                            topic: None,
                            icon: Some("\u{f292}".to_string()),
                        },
                        Channel {
                            id: art_id,
                            name: "Art".to_string(),
                            channel_type: ChannelType::Text,
                            topic: None,
                            icon: Some("\u{f292}".to_string()),
                        },
                    ],
                },
                Category {
                    id: Uuid::new_v4(),
                    name: "VOICE".to_string(),
                    collapsed: false,
                    channels: vec![Channel {
                        id: Uuid::new_v4(),
                        name: "Lounge".to_string(),
                        channel_type: ChannelType::Voice,
                        topic: None,
                        icon: Some("\u{f028}".to_string()),
                    }],
                },
            ],
            created_at: Utc::now(),
        },
        Server {
            id: Uuid::new_v4(),
            name: "Rust Community".to_string(),
            icon_url: None,
            owner_id: Uuid::new_v4(),
            categories: vec![Category {
                id: Uuid::new_v4(),
                name: "GENERAL".to_string(),
                collapsed: false,
                channels: vec![Channel {
                    id: Uuid::new_v4(),
                    name: "welcome".to_string(),
                    channel_type: ChannelType::Text,
                    topic: None,
                    icon: Some("\u{f292}".to_string()),
                }],
            }],
            created_at: Utc::now(),
        },
    ];

    let user_alice = User {
        id: Uuid::new_v4(),
        username: "Alice".to_string(),
        display_name: Some("Alice".to_string()),
        avatar_url: None,
        github_id: Some(12345),
        public_key: None,
        status: UserStatus::Online,
        custom_status: None,
        created_at: Utc::now(),
    };

    let user_bob = User {
        id: Uuid::new_v4(),
        username: "Bob".to_string(),
        display_name: Some("Bob".to_string()),
        avatar_url: None,
        github_id: Some(67890),
        public_key: None,
        status: UserStatus::Online,
        custom_status: Some("Coding Artemis!".to_string()),
        created_at: Utc::now(),
    };

    let user_charlie = User {
        id: Uuid::new_v4(),
        username: "Charlie".to_string(),
        display_name: Some(String::from("Charlie")),
        avatar_url: None,
        github_id: None,
        public_key: None,
        status: UserStatus::Idle,
        custom_status: None,
        created_at: Utc::now(),
    };

    let user_bot = User {
        id: Uuid::new_v4(),
        username: "ArtemisBot".to_string(),
        display_name: Some("ArtemisBot".to_string()),
        avatar_url: None,
        github_id: None,
        public_key: None,
        status: UserStatus::Online,
        custom_status: Some("Idle".to_string()),
        created_at: Utc::now(),
    };

    let messages = vec![
        Message {
            id: Uuid::new_v4(),
            channel_id: general_id,
            author_id: user_alice.id,
            author_name: "Alice".to_string(),
            author_avatar: None,
            content: "Hey everyone! Welcome to Artemis.".to_string(),
            attachments: vec![],
            timestamp: Utc::now() - chrono::Duration::minutes(15),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        },
        Message {
            id: Uuid::new_v4(),
            channel_id: general_id,
            author_id: user_bob.id,
            author_name: "Bob".to_string(),
            author_avatar: None,
            content: "This looks amazing! The UI is really clean.".to_string(),
            attachments: vec![],
            timestamp: Utc::now() - chrono::Duration::minutes(12),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        },
        Message {
            id: Uuid::new_v4(),
            channel_id: general_id,
            author_id: user_alice.id,
            author_name: "Alice".to_string(),
            author_avatar: None,
            content: "Thanks! Built with Rust and iced framework.".to_string(),
            attachments: vec![],
            timestamp: Utc::now() - chrono::Duration::minutes(10),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        },
        Message {
            id: Uuid::new_v4(),
            channel_id: general_id,
            author_id: user_charlie.id,
            author_name: "Charlie".to_string(),
            author_avatar: None,
            content: "Can we get GitHub auth working soon?".to_string(),
            attachments: vec![],
            timestamp: Utc::now() - chrono::Duration::minutes(5),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        },
        Message {
            id: Uuid::new_v4(),
            channel_id: general_id,
            author_id: user_bob.id,
            author_name: "Bob".to_string(),
            author_avatar: None,
            content: "Already on it! The OAuth flow is almost ready.".to_string(),
            attachments: vec![],
            timestamp: Utc::now() - chrono::Duration::minutes(2),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        },
        Message {
            id: Uuid::new_v4(),
            channel_id: general_id,
            author_id: user_bot.id,
            author_name: "ArtemisBot".to_string(),
            author_avatar: None,
            content: "Welcome to #General! Remember to check the rules.".to_string(),
            attachments: vec![],
            timestamp: Utc::now() - chrono::Duration::minutes(1),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        },
    ];

    let members = vec![
        ServerMember {
            user: user_alice,
            role: MemberRole::Founder,
            joined_at: Utc::now(),
        },
        ServerMember {
            user: user_bob,
            role: MemberRole::Moderator,
            joined_at: Utc::now(),
        },
        ServerMember {
            user: user_bot,
            role: MemberRole::Member,
            joined_at: Utc::now(),
        },
        ServerMember {
            user: user_charlie,
            role: MemberRole::Member,
            joined_at: Utc::now(),
        },
    ];

    (servers, messages, members)
}
