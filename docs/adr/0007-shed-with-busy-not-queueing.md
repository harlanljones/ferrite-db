# Shed load with Busy instead of queueing searches

Search admission is bounded by a semaphore (~2× cores in flight); excess calls return `Busy` immediately rather than waiting in a queue. Queueing converts overload into p99 latency violations — exactly the failure Ferrite DB's SLOs forbid — while shedding keeps served requests inside budget. Callers treat `Busy` as a retryable backpressure signal. Expect pressure to enlarge the queue "to make the errors go away"; that trade is the point.

Clarified at G1 (held with FDB-002): `Busy` applies to **search admission only**. Inserts contending for a Table's single writer block on the writer lock; they never return `Busy`.
