-- A thread's engine was checked against a list written into the column, so a
-- new engine could not be stored without rebuilding it. The app validates the
-- name where it reads it (`Engine::parse`), as the server always has: its
-- column carries no list. The column is replaced by one without the check;
-- the touch trigger is set aside so copying the values is not an edit.
DROP TRIGGER agent_threads_updated_at;

ALTER TABLE agent_threads RENAME COLUMN engine TO engine_checked;
ALTER TABLE agent_threads ADD COLUMN engine TEXT NOT NULL DEFAULT 'api';
UPDATE agent_threads SET engine = engine_checked;
ALTER TABLE agent_threads DROP COLUMN engine_checked;

CREATE TRIGGER agent_threads_updated_at
AFTER UPDATE ON agent_threads FOR EACH ROW
WHEN COALESCE((SELECT remote_writes FROM auth_write_admission WHERE singleton = 1), 0) = 0
BEGIN
    UPDATE agent_threads
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
    WHERE id = OLD.id;
END;
