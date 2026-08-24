# Independent Tables, single writer each, no cross-Table queries

One process holds many independent named Tables. Each Table serializes writers behind a lock while searches proceed concurrently — single writer, many readers. Cross-Table queries are permanently out of scope; fan-out across Tables belongs to the caller.

## Consequences

No MVCC, no distributed coordination, and no multi-tenancy guarantees inside Ferrite DB. Applications needing concurrent writers per Table must shard their own writes.
