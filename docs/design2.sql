CREATE TABLE peers (
    id INT AUTOINCREMENT PRIMARY KEY,
    peer_id BYTES NOT NULL UNIQUE,
    commitment_bytes BYTES NOT NULL,
    signature BYTES NOT NULL
);

CREATE TABLE containers (
    id INT AUTOINCREMENT PRIMARY KEY,
    container_id UUID NOT NULL UNIQUE
);

CREATE TABLE logs (
    id INT AUTOINCREMENT PRIMARY KEY,
    peer_id INT NOT NULL REFERENCES peers(id),
    container_id INT NOT NULL REFERENCES containers(id),
    parent_id INT NULL REFERENCES logs(id),
    fork_idx INT NULL,
    fork_hash BYTES NULL,
    ready_idx INT NULL,
    ready_status INT NULL,
    ready_status_data BYTES NULL,
    commit_idx INT NULL,
    commit_status INT NULL,
    commit_status_data BYTES NULL,
    CHECK((fork_idx IS NULL) = (fork_hash IS NULL)),
    CHECK((parent_id IS NOT NULL) OR (fork_idx IS NULL))
);

CREATE UNIQUE INDEX logs_root ON logs(peer_id, container_id) WHERE parent_id IS NULL;

CREATE TABLE segments (
    id INT AUTOINCREMENT PRIMARY KEY,
    log_id INT NOT NULL REFERENCES logs(id),
    start_idx INT NOT NULL,
    end_idx INT NOT NULL,
    end_hash BYTES NOT NULL,
    body BYTES NOT NULL,
);

CREATE INDEX ON segment_log_end segments(log_id, end_idx);
