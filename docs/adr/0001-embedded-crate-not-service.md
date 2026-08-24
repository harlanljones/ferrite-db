# Ferrite DB is an embedded crate, not a service

The original design doc described an "embedded in-process engine" while specifying service-shaped targets (QPS per node). We decided Ferrite DB ships as a Rust crate linked into the host process: performance contracts are per-call latencies at caller-owned concurrency, and any networked wrapper arrives later as a separate crate composing this one. In-process execution — no socket, no serialization — is what makes the p50 budget plausible.
