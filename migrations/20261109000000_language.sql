-- Interface and notification language: en, fr, es or de.
ALTER TABLE settings ADD COLUMN language TEXT NOT NULL DEFAULT 'en';
