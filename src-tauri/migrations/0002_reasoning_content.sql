-- DeepSeek's reasoner family + Claude's extended thinking emit a separate
-- `reasoning_content` field alongside `content` in the streaming response.
-- The provider then *requires* that earlier reasoning_content be echoed
-- back on subsequent turns, or it rejects the request with HTTP 400
-- ("The reasoning_content in the thinking mode must be passed back").
--
-- Persist it per-message so [`build_openai_messages`] can include it when
-- reconstructing the conversation for the next round-trip.
ALTER TABLE messages ADD COLUMN reasoning_content TEXT;
