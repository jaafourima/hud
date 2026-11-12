-- Core server schema. Plugin tables are created by each plugin's server_migrations().

CREATE SCHEMA hud_sys;
CREATE SCHEMA grafana;

-- The role the API switches to (SET LOCAL ROLE) for every data query, so row-level
-- security applies even though the connection user owns the tables.
DO $$ BEGIN
  CREATE ROLE hud_app NOLOGIN;
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $$;
DO $$ BEGIN
  CREATE ROLE hud_grafana NOLOGIN;   -- group: members may read the grafana.* views
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $$;

CREATE TABLE users (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  username      text NOT NULL,
  password_hash text NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_username_lower ON users (lower(username));

CREATE TABLE devices (
  id         uuid PRIMARY KEY,
  user_id    uuid NOT NULL REFERENCES users ON DELETE CASCADE,
  name       text NOT NULL,
  token_hash bytea NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now(),
  last_seen  timestamptz
);

-- Append-only log of every accepted op; clients pull from it by `seq`.
CREATE TABLE ops (
  seq       bigserial PRIMARY KEY,
  user_id   uuid NOT NULL REFERENCES users ON DELETE CASCADE,
  record_id uuid NOT NULL,
  entity    text NOT NULL,
  fields    jsonb NOT NULL,
  hlc       text NOT NULL,
  device_id uuid NOT NULL,
  deleted   boolean NOT NULL,
  UNIQUE (user_id, entity, record_id, hlc)
);
CREATE INDEX ops_pull ON ops (user_id, seq);

CREATE TABLE hud_sys.grafana_access (
  role_name text PRIMARY KEY,
  user_id   uuid NOT NULL REFERENCES users ON DELETE CASCADE
);

CREATE FUNCTION hud_sys.current_user_id() RETURNS uuid
  LANGUAGE sql STABLE
  AS $$ SELECT nullif(current_setting('app.user_id', true), '')::uuid $$;

-- Which user a Grafana login role belongs to. Uses session_user (the login role),
-- which a client cannot change, unlike a custom setting.
CREATE FUNCTION hud_sys.grafana_user() RETURNS uuid
  LANGUAGE sql STABLE SECURITY DEFINER SET search_path = hud_sys, pg_catalog
  AS $$ SELECT user_id FROM hud_sys.grafana_access WHERE role_name = session_user $$;

ALTER TABLE ops ENABLE ROW LEVEL SECURITY;
CREATE POLICY user_isolation ON ops
  USING (user_id = hud_sys.current_user_id()) WITH CHECK (user_id = hud_sys.current_user_id());

GRANT USAGE ON SCHEMA hud_sys TO hud_app;
GRANT EXECUTE ON FUNCTION hud_sys.current_user_id() TO hud_app;
GRANT SELECT, INSERT ON ops TO hud_app;
GRANT USAGE ON SEQUENCE ops_seq_seq TO hud_app;
GRANT USAGE ON SCHEMA grafana TO hud_grafana;
