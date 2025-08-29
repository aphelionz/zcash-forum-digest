-- Add migration script here
CREATE TABLE IF NOT EXISTS topics (
    id BIGINT PRIMARY KEY,
    title TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS posts (
    id BIGINT PRIMARY KEY,
    topic_id BIGINT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    username TEXT NOT NULL,
    cooked TEXT NOT NULL, -- HTML-ish from Discourse
    created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS topic_summaries (
    topic_id BIGINT PRIMARY KEY REFERENCES topics(id) ON DELETE CASCADE,
    summary  TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ingest_cursors (
  name TEXT PRIMARY KEY,
  last_run TIMESTAMPTZ NOT NULL DEFAULT now()
);
