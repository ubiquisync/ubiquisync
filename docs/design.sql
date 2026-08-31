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
    root_hash BYTES NOT NULL,
    last_leaf_hash BYTES, -- the last of the last leaf in this segment (or the prior segment - we should specify which) so that peers can easily check for obvious forks before applying
    encoding BYTES NOT NULL, -- specifies outer compression & encryption if any
    body BYTES NOT NULL,
    signature BYTES NOT NULL, -- can sit out of segment for quick pre-decode verification
    PRIMARY KEY(stream_id, start_idx)
) WITHOUT ROWID;

-- ctl/auth
CREATE TABLE principals (
    id UUID NOT NULL PRIMARY KEY,
    kind INT NOT NULL, -- user, group, workspace
    status BYTES, -- maybe only for users
    PRIMARY KEY
) WITHOUT ROWID;

CREATE TABLE membership_edges (
    parent_id UUID NOT NULL,
    child_id UUID NOT NULL,
    child_kind INT,
    permission INT,
    PRIMARY KEY(parent_id, child_id)
) WITHOUT ROWID;

CREATE TABLE user_devices (
    principal_id UUID NOT NULL,
    peer_id BYTES NOT NULL,
    status BYTES,
    PRIMARY KEY (user_id, peer_id)
);

CREATE TABLE keys (
    id INT PRIMARY KEY,
    fingerprint BYTES NOT NULL UNIQUE,
    container_id BYTES NOT NULL,
    self_wrap BYTES,
    status INT --
    -- maybe link to effects which bound it and invalidated it
);

CREATE TABLE key_recipients (
    key_id INT,
    recipient_id INT,
    PRIMARY KEY (key_id, recipient_id)
);

CREATE TABLE old_keys (
    fingerprint BYTES NOT NULL PRIMARY KEY,
    ciphertext BYTES NOT NULL,
    cipher_key INT REFERENCES keys(id),
    cipher_suite INT
);

CREATE ctl_version_vectors (
    observer_stream_id BYTES NOT NULL,
    observed_stream_id BYTES NOT NULL,  
    idx INT NOT NULL,
    PRIMARY KEY (observer_stream_id,  observed_stream_id )
);

CREATE effect_entries (
    stream_id BYTES NOT NULL,
    idx INT NOT NULL,
    effect_idx INT NOT NULL,
    "timestamp" INT NOT NULL,
    key BYTES,
    value BYTES,
    PRIMARY KEY(stream_id, idx, effect_idx)
);
