-- Add migration script here
CREATE TABLE IF NOT EXISTS Rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule TEXT NOT NULL UNIQUE
);