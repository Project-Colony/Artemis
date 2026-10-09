-- File attachments: 001 already creates the table, so only the upload time
-- is new here.
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS uploaded_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_attachments_message ON attachments(message_id);

-- Full-text search index on messages
CREATE INDEX IF NOT EXISTS idx_messages_fts ON messages USING gin(to_tsvector('english', content));

-- Custom server emojis
CREATE TABLE IF NOT EXISTS custom_emojis (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    server_id UUID NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    name VARCHAR(64) NOT NULL,
    image_url TEXT NOT NULL,
    uploaded_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(server_id, name)
);

CREATE INDEX IF NOT EXISTS idx_custom_emojis_server ON custom_emojis(server_id);
