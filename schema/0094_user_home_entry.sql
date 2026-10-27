-- Home entry is an account preference. NULL means the person has not chosen
-- or skipped the start page. Existing activity still bypasses that page.
-- SQLite cannot add a cross-column CHECK to an existing table. The API accepts
-- choice with any of the five entries, and skip only with clients.
ALTER TABLE users ADD COLUMN home_entry TEXT
    CHECK (home_entry IN ('clients', 'client-market', 'share-market', 'wallet', 'providers'));
ALTER TABLE users ADD COLUMN home_entry_source TEXT
    CHECK (home_entry_source IN ('choice', 'skip'));
