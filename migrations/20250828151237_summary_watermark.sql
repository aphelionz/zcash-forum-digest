-- Add migration script here
ALTER TABLE topic_summaries
  ADD COLUMN IF NOT EXISTS last_post_id BIGINT;
