-- A free note on an incident, written by the admin; shown on status pages when note_public is set.
ALTER TABLE incidents ADD COLUMN note TEXT;
ALTER TABLE incidents ADD COLUMN note_public INTEGER NOT NULL DEFAULT 0;
