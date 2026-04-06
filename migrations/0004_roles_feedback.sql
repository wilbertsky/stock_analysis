-- Add role column to users (admin | subscriber)
ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'subscriber';

-- Feedback submitted by users
CREATE TABLE feedback (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        REFERENCES users(id) ON DELETE SET NULL,
    message    TEXT        NOT NULL CHECK (length(trim(message)) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_read    BOOLEAN     NOT NULL DEFAULT false
);

CREATE INDEX idx_feedback_created_at ON feedback(created_at DESC);
CREATE INDEX idx_feedback_is_read    ON feedback(is_read);
