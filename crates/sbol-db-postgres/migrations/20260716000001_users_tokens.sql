-- SynBioHub-compatible identity: accounts and their API tokens.
--
-- Password hashing and token generation live in the application layer; these
-- tables store the already-computed hashes so a leaked row cannot be replayed.
-- `graph_uri` is the account's owned named graph (http://synbiohub.org/user/
-- <username>) that ACL-scoped reads key on.

CREATE TABLE sbh_user (
    id                   uuid        PRIMARY KEY,
    username             text        NOT NULL UNIQUE,
    name                 text        NOT NULL,
    email                text        NOT NULL UNIQUE,
    affiliation          text,
    password_hash        text        NOT NULL,
    graph_uri            text        NOT NULL,
    is_admin             boolean     NOT NULL DEFAULT false,
    is_curator           boolean     NOT NULL DEFAULT false,
    is_member            boolean     NOT NULL DEFAULT true,
    reset_password_link  text,
    created_at           timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sbh_api_token (
    token_hash  text        PRIMARY KEY,
    user_id     uuid        NOT NULL REFERENCES sbh_user(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- Revoke-by-user and cascade deletes scan by owner.
CREATE INDEX sbh_api_token_user_idx ON sbh_api_token (user_id);
