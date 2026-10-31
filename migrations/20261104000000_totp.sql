-- Optional two-factor authentication (TOTP). totp_secret is base32, NULL = off;
-- totp_last_step is the last time step accepted, so a code is used only once.
ALTER TABLE admin ADD COLUMN totp_secret TEXT;
ALTER TABLE admin ADD COLUMN totp_last_step INTEGER;
