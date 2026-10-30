-- The dashboard groups monitors on this free-text name; NULL = ungrouped. A group exists as long
-- as a monitor uses it.
ALTER TABLE monitors ADD COLUMN group_name TEXT;
