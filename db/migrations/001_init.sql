-- One table per domain. Each has a UNIQUE natural key so re-sent data upserts:
--   INSERT ... ON CONFLICT (<key>) DO UPDATE SET ...
-- `source` records where a row came from (hc_webhook, opengym_json, plees, activitywatch, ...).

CREATE TABLE nutrition (
  id         bigserial PRIMARY KEY,
  date       date        NOT NULL,
  meal       text        NOT NULL,
  calories   numeric,
  protein_g  numeric,
  carbs_g    numeric,
  fat_g      numeric,
  micros     jsonb,
  source     text        NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (date, meal, source)
);

CREATE TABLE body (
  id         bigserial PRIMARY KEY,
  ts         timestamptz NOT NULL,
  weight_kg  numeric,
  body_fat_pct numeric,
  source     text        NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (ts, source)
);

CREATE TABLE workouts (
  id         bigserial PRIMARY KEY,
  start_ts   timestamptz NOT NULL,
  end_ts     timestamptz,
  type       text,
  volume_kg  numeric,
  sets       integer,
  calories   numeric,
  source     text        NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (start_ts, source),
  CHECK (end_ts IS NULL OR end_ts >= start_ts)
);

CREATE TABLE activity (
  id              bigserial PRIMARY KEY,
  date            date    NOT NULL,
  steps           integer,
  active_calories numeric,
  source          text    NOT NULL,
  updated_at      timestamptz NOT NULL DEFAULT now(),
  UNIQUE (date, source)
);

CREATE TABLE sleep (
  id         bigserial PRIMARY KEY,
  start_ts   timestamptz NOT NULL,
  end_ts     timestamptz NOT NULL,
  duration_min integer GENERATED ALWAYS AS
             ((extract(epoch FROM (end_ts - start_ts)) / 60)::integer) STORED,
  source     text        NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (start_ts, source),
  CHECK (end_ts >= start_ts)
);

CREATE TABLE screen_time (
  id         bigserial PRIMARY KEY,
  date       date    NOT NULL,
  device     text    NOT NULL CHECK (device IN ('phone', 'pc')),
  app        text    NOT NULL,
  seconds    integer NOT NULL CHECK (seconds >= 0),
  source     text    NOT NULL DEFAULT 'activitywatch',
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (date, device, app)
);

-- Grafana time-range queries
CREATE INDEX ON body (ts);
CREATE INDEX ON workouts (start_ts);
CREATE INDEX ON sleep (start_ts);
