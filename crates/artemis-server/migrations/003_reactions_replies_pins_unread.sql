-- Discord-like features: reactions, replies, pins, unread tracking, server management

-- Message reactions
CREATE TABLE IF NOT EXISTS message_reactions (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji VARCHAR(64) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(message_id, user_id, emoji)
);

CREATE INDEX IF NOT EXISTS idx_reactions_message ON message_reactions(message_id);

-- Reply-to support (add column to messages)
ALTER TABLE messages ADD COLUMN IF NOT EXISTS reply_to_id UUID REFERENCES messages(id) ON DELETE SET NULL;

-- Pinned messages
CREATE TABLE IF NOT EXISTS pinned_messages (
    channel_id UUID NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    pinned_by UUID NOT NULL REFERENCES users(id),
    pinned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (channel_id, message_id)
);

-- Unread tracking: last read message per user per channel
CREATE TABLE IF NOT EXISTS channel_read_state (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id UUID NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    last_read_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    last_read_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    mention_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, channel_id)
);

CREATE INDEX IF NOT EXISTS idx_read_state_user ON channel_read_state(user_id);
