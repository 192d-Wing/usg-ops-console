#!/bin/sh
# DEV ONLY. Provisions the console DB roles (brief Q1, SEC-006, AU-9). Contract
# from fixes-round-1-gateway.md (migration 0003 raises if these are missing):
#
#   $CONSOLE_OWNER_ROLE    NOLOGIN; owns the DB, schema public, every table
#                          and _sqlx_migrations (migrate pool SET ROLE's to it)
#   $CONSOLE_APP_GROUP     NOLOGIN group; DML on sessions/auth_requests only,
#                          nothing on audit_events (grants come from 0003)
#   $CONSOLE_AUDIT_ROLE    LOGIN; INSERT+SELECT on audit_events only (0002)
#   $CONSOLE_MIGRATOR_ROLE LOGIN; member of owner; used only by migrations
#   $CONSOLE_APP_ROLE      LOGIN runtime role (CONSOLE_DATABASE_URL); member of
#                          the app group; owns nothing, NOCREATEDB/NOCREATEROLE
#
# The runtime role must not own the DB or schema: a schema owner can drop any
# object in it (append-only audit bypass). Integration tests (#[sqlx::test])
# use the superuser URL and provision their own roles.
#
# Runs from docker-entrypoint-initdb.d (fresh volume only: after changing this
# file run `make down-clean`), and from CI against a service container
# (PGHOST/PGPASSWORD set by the caller). Idempotent.
set -eu
: "${CONSOLE_DB_NAME:=console}"
: "${CONSOLE_OWNER_ROLE:=console_owner}"
: "${CONSOLE_APP_GROUP:=console_app}"
: "${CONSOLE_APP_ROLE:=console}"
: "${CONSOLE_APP_PASSWORD:=dev-only-console-db}"
: "${CONSOLE_AUDIT_ROLE:=console_audit}"
: "${CONSOLE_AUDIT_PASSWORD:=dev-only-audit-db}"
: "${CONSOLE_MIGRATOR_ROLE:=console_migrator}"
: "${CONSOLE_MIGRATOR_PASSWORD:=dev-only-migrator-db}"

psql -v ON_ERROR_STOP=1 --username "${POSTGRES_USER:-postgres}" --dbname "${POSTGRES_DB:-postgres}" \
  -v owner="$CONSOLE_OWNER_ROLE" -v appgrp="$CONSOLE_APP_GROUP" \
  -v app="$CONSOLE_APP_ROLE" -v app_pw="$CONSOLE_APP_PASSWORD" \
  -v audit="$CONSOLE_AUDIT_ROLE" -v audit_pw="$CONSOLE_AUDIT_PASSWORD" \
  -v mig="$CONSOLE_MIGRATOR_ROLE" -v mig_pw="$CONSOLE_MIGRATOR_PASSWORD" \
  -v db="$CONSOLE_DB_NAME" <<'SQL'
SELECT format('CREATE ROLE %I NOLOGIN', :'owner')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'owner') \gexec
SELECT format('CREATE ROLE %I NOLOGIN', :'appgrp')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'appgrp') \gexec
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOCREATEDB NOCREATEROLE', :'audit', :'audit_pw')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'audit') \gexec
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOCREATEDB NOCREATEROLE', :'mig', :'mig_pw')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'mig') \gexec
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOCREATEDB NOCREATEROLE', :'app', :'app_pw')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'app') \gexec
-- Enforce attributes even if a role pre-existed (e.g. old dev volume).
SELECT format('ALTER ROLE %I NOLOGIN', :'owner') \gexec
SELECT format('ALTER ROLE %I NOLOGIN', :'appgrp') \gexec
SELECT format('ALTER ROLE %I NOCREATEDB NOCREATEROLE NOSUPERUSER', :'app') \gexec
SELECT format('GRANT %I TO %I', :'owner', :'mig') \gexec
SELECT format('GRANT %I TO %I', :'appgrp', :'app') \gexec
SELECT format('CREATE DATABASE %I OWNER %I', :'db', :'owner')
 WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = :'db') \gexec
SELECT format('ALTER DATABASE %I OWNER TO %I', :'db', :'owner') \gexec
SELECT format('REVOKE ALL ON DATABASE %I FROM PUBLIC', :'db') \gexec
SELECT format('GRANT CONNECT ON DATABASE %I TO %I, %I, %I', :'db', :'mig', :'app', :'audit') \gexec
\connect :db
SELECT format('ALTER SCHEMA public OWNER TO %I', :'owner') \gexec
REVOKE ALL ON SCHEMA public FROM PUBLIC;
SELECT format('GRANT USAGE ON SCHEMA public TO %I, %I', :'appgrp', :'audit') \gexec
SQL
