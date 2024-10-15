-- Add migration script here
CREATE TABLE IF NOT EXISTS content (
    `id`           INTEGER PRIMARY KEY NOT NULL,
    `subject`      VARCHAR(100)        NOT NULL,
    `linkDownload` VARCHAR(255)        NOT NULL
);
