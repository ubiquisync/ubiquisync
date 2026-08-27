CREATE TABLE peers (
    id INT AUTOINCREMENT,
    peer_id BYTES NOT NULL UNIQUE,
    commitment_bytes BYTES NOT NULL,
    signature BYTES NOT NULL
) WITHOUT ROWID;

CREATE TABLE segments (
    stream_id BYTES NOT NULL,
    start_idx INT NOT NULL,
    peer_id INT NOT NULL,
    container_id UUID NOT NULL,
    end_idx INT NOT NULL
    end_hash BYTES NOT NULL,
    body BYTES NOT NULL,
    status BYTES NULL
    PRIMARY KEY(stream_id, start_idx)
);
