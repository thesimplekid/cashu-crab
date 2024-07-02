
-- Proof Table
CREATE TABLE IF NOT EXISTS proof (
y BLOB PRIMARY KEY,
amount INTEGER NOT NULL,
keyset_id TEXT NOT NULL,
secret TEXT NOT NULL,
c BLOB NOT NULL,
witness TEXT,
state TEXT CHECK ( state IN ('SPENT', 'PENDING' ) ) NOT NULL
);

CREATE INDEX IF NOT EXISTS state_index ON proof(state);
CREATE INDEX IF NOT EXISTS secret_index ON proof(secret);

-- Keysets Table

CREATE TABLE IF NOT EXISTS keyset (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    valid_from INTEGER NOT NULL,
    valid_to INTEGER,
    derivation_path TEXT NOT NULL,
    max_order INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS unit_index ON keyset(unit);
CREATE INDEX IF NOT EXISTS active_index ON keyset(active);


CREATE TABLE IF NOT EXISTS mint_quote (
    id TEXT PRIMARY KEY,
    mint_url TEXT NOT NULL,
    amount INTEGER NOT NULL,
    unit TEXT NOT NULL,
    request TEXT NOT NULL,
    paid BOOL NOT NULL DEFAULT FALSE,
    expiry INTEGER NOT NULL
);


CREATE INDEX IF NOT EXISTS paid_index ON mint_quote(paid);
CREATE INDEX IF NOT EXISTS request_index ON mint_quote(request);
CREATE INDEX IF NOT EXISTS expiry_index ON mint_quote(expiry);

CREATE TABLE IF NOT EXISTS melt_quote (
    id TEXT PRIMARY KEY,
    unit TEXT NOT NULL,
    amount INTEGER NOT NULL,
    request TEXT NOT NULL,
    fee_reserve INTEGER NOT NULL,
    paid BOOL NOT NULL DEFAULT FALSE,
    expiry INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS paid_index ON melt_quote(paid);
CREATE INDEX IF NOT EXISTS request_index ON melt_quote(request);
CREATE INDEX IF NOT EXISTS expiry_index ON melt_quote(expiry);

CREATE TABLE IF NOT EXISTS blind_signature (
    y BLOB PRIMARY KEY,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    c BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS keyset_id_index ON blind_signature(keyset_id);
