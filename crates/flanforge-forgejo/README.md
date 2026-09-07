# flanforge-forgejo

Narrow Forgejo API client for ephemeral runners and their waiting jobs.

- Owns the ephemeral runner lifecycle for one repository: create, read status,
  delete, and reconcile away every runner left under a deterministic name.
- Owns waiting-job observation: binds at most one listed job to the signed
  `run_id` and configured job name, and diagnoses why a labelled search keeps
  coming back empty instead of waiting the allocation out.
- Bounds and validates every remote response — size-capped bodies, a listing
  page budget, structural validation — before returning a typed model.
- Reads the API token from a private regular file of bounded contents, holds it
  in a wrapper that cannot be printed, and redacts issued runner credentials in
  `Debug`; no error variant carries a secret.
- Classifies a failure as retryable, absent, or a rejection, but neither retries
  nor schedules: polling and backoff belong to the caller.
