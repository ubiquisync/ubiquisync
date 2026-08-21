CREATE TABLE peers (
    peer_id BYTES NOT NULL,
    genesis_bytes BYTES,
    status BYTES
) WITHOUT ROWID;

CREATE TABLE streams (
    id BYTES NOT NULL PRIMARY KEY,
    peer_id BYTES NOT NULL,
    container_id UUID NOT NULL,
    parent_id INT NULL REFERENCES id,
    horizon_info BYTES NULL, -- horizon index + MMR peaks
    received_idx INT NOT NULL DEFAULT 0,
    received_mmr_peaks BYTES NULL, -- MMR peaks at received_idx
    active_cipher BYTES NULL, -- encryption key fingerprint + cipher suite at received idx
    processed_idx INT NOT NULL DEFAULT 0,
    committed_idx INT NOT NULL DEFAULT 0,
    status BYTES, -- awaiting cipher key, awaiting upgrade (entry type, cipher suite), stalled on hlc skew, other stall (should distinguish waiting vs unrecoverable)
);

CREATE TABLE segments (
    stream_id INT NOT NULL,
    start_idx INT NOT NULL,
    size INT NOT NULL
    encoding INT NOT NULL,
    body BYTES NOT NULL,
    PRIMARY KEY(stream_id, start_idx)
) WITHOUT ROWID;

-- ctl/auth
CREATE TABLE users (
    user_id UUID NOT NULL PRIMARY KEY,
    status BYTES,
    PRIMARY KEY
);

CREATE TABLE user_devices (
    user_id UUID NOT NULL,
    peer_id BYTES NOT NULL,
    PRIMARY KEY (user_id, peer_id)
);

CREATE ctl_version_vectors (
    observer_stream_id BYTES NOT NULL,
    observed_stream_id BYTES NOT NULL,  
    idx INT NOT NULL,
    PRIMARY KEY (observer_stream_id,  observed_stream_id )
);

CREATE effect_entries (
    id BIGSERIAL,
    stream_id BYTES NOT NULL,
    idx INT NOT NULL,
    root_hash BYTES NOT NULL,
    UNIQUE(stream_id, idx, root)
);

CREATE effect (
    id BIGSERIAL
    entry_id INT NOT NULL,
    op_idx INT NOT NULL,
    effect BYTES NOT NULL,
    revokes id NULL
);
