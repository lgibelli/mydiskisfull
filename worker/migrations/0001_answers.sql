-- What Claude said of each folder, by its name as asked (lowercased: APFS
-- doesn't tell case apart). Nothing about who asked.
CREATE TABLE answers (
  key TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  known INTEGER NOT NULL,
  what TEXT,
  grows TEXT,
  helps TEXT,
  model TEXT NOT NULL,
  -- The first asker's macOS version: system folders change between them.
  macos TEXT NOT NULL,
  created INTEGER NOT NULL,
  -- How often it was asked, for the review (the most asked go into folders.json).
  asked INTEGER NOT NULL DEFAULT 1
);

-- Questions Claude answered each day (UTC), for the daily cap.
CREATE TABLE days (
  day TEXT PRIMARY KEY,
  asks INTEGER NOT NULL
);
