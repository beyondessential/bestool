CREATE TABLE blobs (
	id varchar(255) NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
	created_at timestamptz DEFAULT now(),
	updated_at timestamptz DEFAULT now(),
	deleted_at timestamptz,
	hash text NOT NULL,
	size bigint NOT NULL,
	integrity_state text NOT NULL DEFAULT 'verified',
	tier text NOT NULL DEFAULT 'cache',
	last_accessed_at timestamptz NOT NULL DEFAULT now(),
	eligible_since_tick bigint,
	last_scrubbed_at timestamptz,
	has_parity boolean NOT NULL DEFAULT false,
	correction_count integer NOT NULL DEFAULT 0,
	last_corrected_at timestamptz,
	scan_verdict text,
	scanned_at timestamptz,
	scanner_version text,
	signature_version text
);
CREATE UNIQUE INDEX blobs_hash ON blobs (hash);

CREATE TABLE blob_quarantines (
	id text NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
	created_at timestamptz NOT NULL DEFAULT now(),
	updated_at timestamptz NOT NULL DEFAULT now(),
	deleted_at timestamptz,
	hash text NOT NULL,
	scanner_version text,
	signature_version text,
	CONSTRAINT blob_quarantines_hash UNIQUE (hash) DEFERRABLE INITIALLY IMMEDIATE
);

CREATE TABLE settings (
	id uuid NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
	created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
	updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
	deleted_at timestamptz,
	key text NOT NULL,
	value jsonb,
	facility_id varchar(255),
	scope text NOT NULL DEFAULT 'global'
);

CREATE TABLE local_system_facts (
	id varchar(255) NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
	created_at timestamptz DEFAULT now(),
	updated_at timestamptz DEFAULT now(),
	deleted_at timestamptz,
	key varchar(255) NOT NULL UNIQUE,
	value text
);
