-- Vitess Share Events Schema
--
-- This schema defines the share_events table for storing mining share submissions
-- in a Vitess-sharded database. The table is designed for horizontal scalability
-- using user_identity as the sharding key.
--
-- Apply this schema to your Vitess cluster using:
--   vtctlclient -server vtctld:15999 ApplySchema -sql-file schema.sql mining_pool
--
-- Or for vttablet direct access during development:
--   mysql -h vttablet -P 3306 -u root < schema.sql

CREATE TABLE IF NOT EXISTS share_events (
    -- Primary key and sharding
    -- 'id' is auto-incremented within each shard
    -- 'shard_key' is used by Vitess for routing (matches user_identity)
    id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    shard_key VARCHAR(255) NOT NULL,

    -- Temporal data
    -- 'event_timestamp' stores Unix timestamp in microseconds for precise ordering
    -- 'inserted_at' tracks when the row was created in the database
    event_timestamp BIGINT UNSIGNED NOT NULL,
    inserted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    -- Share identification
    -- 'user_identity' identifies the miner (e.g., wallet address or username)
    -- 'template_id' references the mining template used for this share
    user_identity VARCHAR(255) NOT NULL,
    template_id BIGINT UNSIGNED,

    -- Share validation results
    -- 'is_valid' indicates if the share met difficulty requirements
    -- 'is_block_found' indicates if this share solved a block
    -- 'error_code' contains rejection reason for invalid shares (NULL if valid)
    is_valid BOOLEAN NOT NULL,
    is_block_found BOOLEAN NOT NULL DEFAULT FALSE,
    error_code VARCHAR(64),

    -- Share cryptographic data
    -- 'share_hash' is the SHA256d hash of the share (optional, may be NULL)
    -- 'target' is the difficulty target as 32-byte big-endian value
    -- 'nonce' is the nonce value that produced this hash
    -- 'version' is the block version field
    -- 'ntime' is the block timestamp (Unix seconds)
    share_hash BINARY(32),
    target BINARY(32) NOT NULL,
    nonce INT UNSIGNED NOT NULL,
    version INT UNSIGNED NOT NULL,
    ntime INT UNSIGNED NOT NULL,

    -- Mining metadata
    -- 'extranonce_prefix' is the miner's assigned extranonce space
    -- 'rollable_extranonce_size' indicates how many bytes can be modified
    extranonce_prefix VARBINARY(255) NOT NULL,
    rollable_extranonce_size SMALLINT UNSIGNED,

    -- Performance metrics
    -- 'share_work' is the difficulty value of this share (double precision)
    -- 'nominal_hash_rate' is the estimated hash rate for this submission
    share_work DOUBLE NOT NULL,
    nominal_hash_rate FLOAT NOT NULL,

    -- Primary key includes shard_key for Vitess routing
    PRIMARY KEY (id, shard_key),

    -- Indexes optimized for common query patterns
    -- Most queries filter by user for payout calculations and stats
    KEY idx_user_timestamp (user_identity, event_timestamp),
    -- Global time-range queries for admin dashboards
    KEY idx_timestamp (event_timestamp),
    -- Quick lookups for block discoveries
    KEY idx_block_found (is_block_found, event_timestamp),
    -- Payout calculations (valid shares by user over time period)
    KEY idx_valid_user (is_valid, user_identity, event_timestamp),
    -- Mining efficiency analysis (shares per template)
    KEY idx_template (template_id, event_timestamp)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

-- Note: For Vitess deployments, ensure your VSchema (vschema.json) is configured
-- to shard this table using the 'shard_key' column with a hash vindex.
