-- Add migration script here
CREATE TABLE IF NOT EXISTS topic_summaries_llm (
  topic_id      BIGINT PRIMARY KEY REFERENCES topics(id) ON DELETE CASCADE,
  summary       TEXT NOT NULL,
  model         TEXT NOT NULL,
  prompt_hash   TEXT NOT NULL,
  input_tokens  INTEGER,
  output_tokens INTEGER,
  cost_usd      NUMERIC,
  updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- optional index that helps lookups by recency
CREATE INDEX IF NOT EXISTS topic_summaries_llm_updated_idx ON topic_summaries_llm(updated_at DESC);
