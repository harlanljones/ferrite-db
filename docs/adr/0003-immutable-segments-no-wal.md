# Immutable segments with atomic-rename commits, no WAL

The write path appends immutable Segments that become visible via atomic rename; there is deliberately no write-ahead log. A crash loses only in-flight batches — anything further is recovered by re-ingesting from upstream, which the target workload (derived embeddings) makes cheap.

## Consequences

Callers that cannot tolerate losing an accepted batch must hold their own source back until Commit confirmation returns. Expect someone to propose adding a WAL; this record is why it is absent.
