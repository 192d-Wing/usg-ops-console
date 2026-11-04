-- DEV/CI evidence for SEC-006 / AU-9: run as superuser against the console DB
-- AFTER migrations. Raises (psql exit 3 with ON_ERROR_STOP) on any violation.
--   psql -v ON_ERROR_STOP=1 -d console -f deploy/compose/postgres/check-roles.sql
DO $$
DECLARE bad text;
BEGIN
  IF (SELECT rolcanlogin FROM pg_roles WHERE rolname = 'console_owner') IS DISTINCT FROM false THEN
    RAISE EXCEPTION 'console_owner must exist and be NOLOGIN';
  END IF;
  IF (SELECT rolcanlogin FROM pg_roles WHERE rolname = 'console_app') IS DISTINCT FROM false THEN
    RAISE EXCEPTION 'console_app must exist and be NOLOGIN';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console' AND (rolcreatedb OR rolcreaterole OR rolsuper)) THEN
    RAISE EXCEPTION 'runtime role console must be NOCREATEDB NOCREATEROLE NOSUPERUSER';
  END IF;
  IF pg_has_role('console', 'console_owner', 'MEMBER') THEN
    RAISE EXCEPTION 'runtime role console must not be a member of console_owner';
  END IF;
  SELECT string_agg(format('%s owned by %s', c.relname, pg_get_userbyid(c.relowner)), ', ') INTO bad
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
   WHERE n.nspname = 'public' AND c.relkind IN ('r','p','S','v','m','i')
     AND pg_get_userbyid(c.relowner) <> 'console_owner';
  IF bad IS NOT NULL THEN RAISE EXCEPTION 'objects not owned by console_owner: %', bad; END IF;
  IF (SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = current_database()) <> 'console_owner' THEN
    RAISE EXCEPTION 'database not owned by console_owner';
  END IF;
  IF (SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname = 'public') <> 'console_owner' THEN
    RAISE EXCEPTION 'schema public not owned by console_owner';
  END IF;
  IF has_table_privilege('console', 'audit_events', 'INSERT') OR has_table_privilege('console', 'audit_events', 'SELECT')
     OR has_table_privilege('console', 'audit_events', 'UPDATE') OR has_table_privilege('console', 'audit_events', 'DELETE')
     OR has_table_privilege('console', 'audit_events', 'TRUNCATE') THEN
    RAISE EXCEPTION 'runtime role console must have no rights on audit_events';
  END IF;
  IF NOT (has_table_privilege('console', 'sessions', 'SELECT,INSERT,UPDATE,DELETE')
          AND has_table_privilege('console', 'auth_requests', 'SELECT,INSERT,UPDATE,DELETE')) THEN
    RAISE EXCEPTION 'runtime role console lacks DML on sessions/auth_requests';
  END IF;
  IF has_table_privilege('console_audit', 'audit_events', 'UPDATE') OR has_table_privilege('console_audit', 'audit_events', 'DELETE')
     OR has_table_privilege('console_audit', 'sessions', 'SELECT') OR NOT has_table_privilege('console_audit', 'audit_events', 'INSERT,SELECT') THEN
    RAISE EXCEPTION 'console_audit must have exactly INSERT+SELECT on audit_events';
  END IF;
  -- NIT-3: the runtime app group (and its login) must not be able to rewrite
  -- migration history (forging checksums would let a tampered schema pass).
  IF to_regclass('public._sqlx_migrations') IS NULL THEN
    RAISE EXCEPTION '_sqlx_migrations missing: run console-gateway migrate first';
  END IF;
  IF has_table_privilege('console_app', '_sqlx_migrations', 'INSERT') OR has_table_privilege('console_app', '_sqlx_migrations', 'UPDATE')
     OR has_table_privilege('console_app', '_sqlx_migrations', 'DELETE') OR has_table_privilege('console_app', '_sqlx_migrations', 'TRUNCATE')
     OR has_table_privilege('console', '_sqlx_migrations', 'INSERT') OR has_table_privilege('console', '_sqlx_migrations', 'UPDATE')
     OR has_table_privilege('console', '_sqlx_migrations', 'DELETE') OR has_table_privilege('console', '_sqlx_migrations', 'TRUNCATE') THEN
    RAISE EXCEPTION 'console_app/console must not write _sqlx_migrations';
  END IF;
  RAISE NOTICE 'SEC-006 role contract OK';
END $$;
