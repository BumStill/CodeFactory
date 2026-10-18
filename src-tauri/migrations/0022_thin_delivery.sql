-- R1/R2: historical sessions stay legacy; new-session entry points opt into thin.
ALTER TABLE sessions ADD COLUMN delivery_mode TEXT NOT NULL DEFAULT 'legacy' CHECK (delivery_mode IN ('thin', 'legacy'));
CREATE TABLE IF NOT EXISTS thin_delivery_refs (
    session_id TEXT PRIMARY KEY,
    branch TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    ceiling TEXT NOT NULL
);
