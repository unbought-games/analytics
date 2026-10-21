-- External access tokens have local revocation records, preserving authorization across queued work.
ALTER TABLE credentials ADD COLUMN oidc_issuer text;
ALTER TABLE credentials ADD COLUMN oidc_subject text;
ALTER TABLE credentials ADD CONSTRAINT oidc_pair CHECK ((oidc_issuer IS NULL) = (oidc_subject IS NULL));
CREATE INDEX credentials_oidc_binding ON credentials(oidc_issuer,oidc_subject) WHERE oidc_issuer IS NOT NULL;
