# Event-store migrations

Production uses PostgreSQL. `apps/api/migrations/0001_event_store.up.sql` creates the first event-store schema and the matching `.down.sql` reverses it for development/test rollback. Production rollback of a migration that has accepted sessions requires an explicit backup/export and maintenance window; completed-run event history is never silently discarded.

Application code may call `metadata.create_all()` only for isolated tests and disposable local stores. Deployed environments apply versioned SQL migrations before application startup. Forward schema changes must preserve append-only command/event history or provide an explicit copy/migration step.
